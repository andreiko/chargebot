use tokio::{
    select,
    sync::{mpsc, Mutex},
};

use super::{
    client::Client,
    models::{GetUpdates, Update},
};
use crate::utils::retry::{exp_backoff_forever, retry};

/// Contains configuration options for the Telegram update poller.
pub struct Config {
    pub output: mpsc::Sender<Update>,
    pub duration_seconds: u16,
    pub cancel: mpsc::Receiver<()>,
}

/// Starts the update poller.
///
/// It fetches updates from Telegram API via `Client` using the long polling technique
/// and sends the received updates to the `output` channel.
///
/// Poller will shut down when the `cancel` channel is closed or when it encounters an unrecoverable error.
pub async fn start(client: impl Client, cfg: Config) {
    let mut next_offset = 0u64;
    let cancel = Mutex::new(cfg.cancel);

    loop {
        let updates_res = retry(
            &mut exp_backoff_forever(),
            || async {
                let get_updates_fut = client.get_updates(GetUpdates {
                    offset: next_offset as i64,
                    timeout: cfg.duration_seconds,
                });
                let cancel = &mut cancel.lock().await;
                select! {
                    // propagate the result but wrap in Option
                    result = get_updates_fut => result.map(Some),
                    // return Ok(None) if we receive cancellation during an attempt
                    _ = cancel.recv() => Ok(None),
                }
            },
            |e| e.is_server() || e.is_transport(),
            |e| {
                log::error!(
                    "transient error while polling updates (will be retried): {}",
                    e
                )
            },
        )
        .await;

        let updates = match updates_res {
            Ok(updates_opt) => match updates_opt {
                Some(updates) => updates,
                None => return, // stop polling if got cancelled during the attempt
            },
            Err(err) => {
                return log::error!("polling manager failed to get updates: {}", err);
            }
        };

        for update in updates {
            if update.update_id >= next_offset {
                next_offset = update.update_id + 1;
            }

            if let Err(err) = cfg.output.send(update).await {
                return log::error!("polling manager failed to send update: {}", err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde::Serialize;
    use tokio::sync::mpsc;

    use super::{
        super::{
            client::Client,
            models::{GetUpdates, Payload, Update},
        },
        start, Config,
    };
    use crate::utils::{
        error::Error,
        testing::{expect_no_recv, expect_recv},
    };

    struct MockClient {
        confirmed_id: i64,
        updates: Vec<Update>,
    }

    impl Default for MockClient {
        fn default() -> Self {
            Self {
                confirmed_id: 0,
                updates: Vec::default(),
            }
        }
    }

    impl MockClient {
        fn add_update(&mut self, update: Update) {
            self.updates.push(update);
        }
    }

    #[async_trait]
    impl Client for Arc<Mutex<MockClient>> {
        async fn deliver_payload<P: Payload + Serialize + Sync>(&self, _: &P) -> Result<(), Error> {
            panic!("unexpected call to deliver_payload()")
        }

        async fn get_updates(&self, get_updates: GetUpdates) -> Result<Vec<Update>, Error> {
            let mut locked = self.lock().unwrap();
            locked.confirmed_id = get_updates.offset;
            Ok(locked
                .updates
                .iter()
                .flat_map(|u| {
                    if u.update_id >= get_updates.offset as u64 {
                        Some(u.clone())
                    } else {
                        None
                    }
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn happy_flow() {
        let (updates_tx, mut updates_rx) = mpsc::channel::<Update>(10);
        let client = Arc::new(Mutex::new(MockClient::default()));
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

        // add some updates so that they'd be available immediately after poller starts
        client.lock().unwrap().add_update(Update::new(1234));
        client.lock().unwrap().add_update(Update::new(1235));

        // start the poller
        let handle = tokio::spawn(start(
            client.clone(),
            Config {
                output: updates_tx,
                duration_seconds: 30,
                cancel: cancel_rx,
            },
        ));

        // receive the prepopulated updates
        let u = expect_recv(&mut updates_rx).await.unwrap();
        assert_eq!(u.update_id, 1234);
        let u = expect_recv(&mut updates_rx).await.unwrap();
        assert_eq!(u.update_id, 1235);

        // nothing to receive anymore
        expect_no_recv(&mut updates_rx).await;

        // make another update available and receive it on the channel
        client.lock().unwrap().add_update(Update::new(1300));
        let u = expect_recv(&mut updates_rx).await.unwrap();
        assert_eq!(u.update_id, 1300);

        // nothing to receive anymore
        expect_no_recv(&mut updates_rx).await;

        // close the cancellation channel and expect poller to finish
        drop(cancel_tx);
        handle.await.unwrap();
    }
}
