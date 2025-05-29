use std::{
    collections::HashMap,
    ops::{Deref, RangeInclusive},
    sync::Arc,
    time::Duration,
};

use rand::Rng;
use tokio::{
    select,
    sync::{mpsc, oneshot},
    time::sleep,
};

use super::{
    client::Client,
    models::{Station, Status},
};
use crate::{
    logging::log_error_with_backtrace,
    utils::retry::{exp_backoff_forever, retry},
    whatever::{SnafuExt, WhateverSync},
};

/// Contains configuration options for the watcher.
pub struct Config {
    pub input: mpsc::Receiver<Command>,
    pub output: mpsc::Sender<Station>,
    pub poll_interval: RangeInclusive<Duration>,
}

/// Starts the park watch manager.
///
/// It can be controlled by sending commands to the input channel:
/// SubscribeToPark(ID):     starts a task polling a Park with the specified ID if it hasn't existed.
///                          The new task task immediately fetches all of the park's station statuses
///                          and pushes to the output channel. Then it polls the park at random time
///                          intervals specified in `cfg.poll_interval`, sending updates when there
///                          are changes.
/// UnsubscribeFromPark(ID): removes the subscription from a Park with the specified ID and stops
///                          the corresponding task. Nothing gets sent to the output in response.
///
/// Watch manager will shut down when `input` is closed or when one of the watcher tasks encounters
/// an unrecoverable error.
pub async fn start(client: impl Client + Sync + Send + 'static, cfg: Config) {
    let output = Arc::new(cfg.output);
    let mut input = cfg.input;

    let client = Arc::new(client);
    let mut watchers = HashMap::<String, oneshot::Sender<()>>::new();
    let (fail_tx, mut fail_rx) = mpsc::channel::<()>(1);
    let fail_tx = Arc::new(fail_tx);
    loop {
        select! {
            cmd_opt = input.recv() => {
                let Some(cmd) = cmd_opt else { break };

                match cmd {
                    Command::SubscribeToPark(park_id) => {
                        if watchers.contains_key(&park_id) {
                            continue;
                        }

                        let (done_tx, done_rx) = oneshot::channel::<()>();
                        watchers.insert(park_id.clone(), done_tx);
                        tracing::debug!("watcher subscribed to new park {park_id}");
                        tokio::spawn(watch_park(park_id, client.clone(), output.clone(),
                            cfg.poll_interval.clone(), done_rx, fail_tx.clone()));
                    }
                    Command::UnsubscribeFromPark(park_id) => {
                        tracing::debug!("watcher unsubscribed from park {park_id}");
                        watchers.remove(&park_id);
                    }
                }
            }
            _ = fail_rx.recv() => {
                tracing::error!("watch manager failed due to previous park watcher failure");
                return;
            }
        }
    }

    tracing::debug!("watch manager finished");
}

/// Starts a task that watches a Park with the specified `id` and sends updates to the `output` channel.
///
/// The task shuts down when `done` is closed.
/// On an unrecoverable error, it sends () to the `fail` channel, indicating that park watch manager
/// must shut down.
async fn watch_park<C: Client + Sync + Send>(
    id: String,
    client: Arc<C>,
    output: Arc<mpsc::Sender<Station>>,
    poll_interval: RangeInclusive<Duration>,
    mut done: oneshot::Receiver<()>,
    fail: Arc<mpsc::Sender<()>>,
) {
    tracing::debug!("starting park watcher {id}");
    let mut statuses = HashMap::<String, Status>::new();

    if let Err(err) = refresh_park(&id, &mut statuses, client.clone(), output.clone()).await {
        fail.send(()).await.unwrap();
        log_error_with_backtrace(format!("park watcher {id} failed"), &err);
        return;
    }

    loop {
        select! {
            _ = sleep(rand::rng().random_range(poll_interval.clone())) => {
                if let Err(err) = refresh_park(&id, &mut statuses, client.clone(), output.clone()).await {
                    fail.send(()).await.unwrap();
                    log_error_with_backtrace(format!("park watcher {id} failed"), &err);
                    return;
                }
            }
            _ = &mut done => break
        }
    }

    tracing::debug!("park watcher {id} finished");
}

/// Attempts to fetch status of a park with the specified ID and send updates for statuses that
/// haven't been known before or changed since the last check. Retries *transient* errors indefinitely.
async fn refresh_park<C: Client>(
    park_id: &str,
    statuses: &mut HashMap<String, Status>,
    client: Arc<C>,
    tx: Arc<mpsc::Sender<Station>>,
) -> Result<(), WhateverSync> {
    let park = retry(
        exp_backoff_forever,
        || client.get_park(park_id),
        |e| log_error_with_backtrace("transient error while fetching park (will be retried)", &e),
    )
    .await
    .whatever()?;

    for s in park.stations {
        let should_send = match statuses.get_mut(&s.id) {
            Some(known) => {
                if known.deref().ne(&s.status) {
                    *known = s.status.clone();
                    true
                } else {
                    false
                }
            }
            None => {
                statuses.insert(s.id.clone(), s.status.clone());
                true
            }
        };

        if should_send {
            // if send fails here, it means that an unrecoverable error happened in
            // another component and the program is shutting down.
            tx.send(s).await.whatever()?;
        }
    }

    Ok(())
}

/// Represents commands that can be used to control Park watch manager.
pub enum Command {
    SubscribeToPark(String),
    UnsubscribeFromPark(String),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use super::{
        super::{
            client::Client,
            models::{Park, Station, Status},
        },
        start, Command, Config,
    };
    use crate::utils::{
        error::Error,
        testing::{expect_no_recv, expect_recv},
    };

    struct MockClient {
        park_map: HashMap<String, Park>,
    }

    #[async_trait]
    impl Client for Arc<Mutex<MockClient>> {
        async fn get_park(&self, park_id: &str) -> Result<Park, Error> {
            self.lock()
                .unwrap()
                .park_map
                .get(park_id)
                .map(|p| p.clone())
                .ok_or(Error::UnexpectedStatus { code: 404 })
        }
    }

    impl MockClient {
        pub fn new(parks: impl IntoIterator<Item = Park>) -> Self {
            Self {
                park_map: parks.into_iter().map(|p| (p.id.clone(), p)).collect(),
            }
        }

        fn set_status(&mut self, park_id: &str, station_id: &str, new_status: Status) {
            if let Some(park) = self.park_map.get_mut(park_id) {
                for station in &mut park.stations {
                    if station.id == station_id {
                        station.status = new_status;
                        return;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn happy_flow() {
        let client = Arc::new(Mutex::new(MockClient::new(vec![
            Park {
                id: "p1".to_string(),
                stations: vec![
                    Station {
                        id: "p1s1".to_string(),
                        status: Status::InUse,
                    },
                    Station {
                        id: "p1s2".to_string(),
                        status: Status::Available,
                    },
                ],
            },
            Park {
                id: "p2".to_string(),
                stations: vec![Station {
                    id: "p2s1".to_string(),
                    status: Status::InUse,
                }],
            },
        ])));

        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(10);
        let (stations_tx, mut stations_rx) = mpsc::channel::<Station>(10);

        let handle = tokio::spawn(start(
            client.clone(),
            Config {
                input: cmd_rx,
                output: stations_tx,
                poll_interval: Duration::from_millis(1)..=Duration::from_millis(5),
            },
        ));

        // subscribe to P2, expect an immediate update with current status
        cmd_tx
            .send(Command::SubscribeToPark("p2".to_string()))
            .await
            .unwrap();
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p2s1");
        assert!(matches!(station.status, Status::InUse));

        // subscribe to P2 again, nothing is expected in return as the request is deduplicated
        cmd_tx
            .send(Command::SubscribeToPark("p2".to_string()))
            .await
            .unwrap();
        expect_no_recv(&mut stations_rx).await;

        // subscribe to P1, expect immediate updates with current status, one for each station in P1
        cmd_tx
            .send(Command::SubscribeToPark("p1".to_string()))
            .await
            .unwrap();
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p1s1");
        assert!(matches!(station.status, Status::InUse));
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p1s2");
        assert!(matches!(station.status, Status::Available));

        // modify P1S2 state, expect an update
        client
            .lock()
            .unwrap()
            .set_status("p1", "p1s2", Status::InUse);
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p1s2");
        assert!(matches!(station.status, Status::InUse));

        // modify P2S1 status, expect an update
        client
            .lock()
            .unwrap()
            .set_status("p2", "p2s1", Status::Available);
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p2s1");
        assert!(matches!(station.status, Status::Available));

        // unsubscribe from P1, nothing is expected in return
        cmd_tx
            .send(Command::UnsubscribeFromPark("p1".to_string()))
            .await
            .unwrap();
        expect_no_recv(&mut stations_rx).await;

        // flip all station statuses in P1, nothing is expected in return as we're not subscribed to it
        {
            let mut client = client.lock().unwrap();
            client.set_status("p1", "p1s1", Status::Available);
            client.set_status("p1", "p1s2", Status::Available);
        }
        expect_no_recv(&mut stations_rx).await;

        // modify P2S1 status, expect an update
        client
            .lock()
            .unwrap()
            .set_status("p2", "p2s1", Status::InUse);
        let station = expect_recv(&mut stations_rx).await.unwrap();
        assert_eq!(station.id, "p2s1");
        assert!(matches!(station.status, Status::InUse));

        // close the command channel and expect watcher to finish
        drop(cmd_tx);
        handle.await.unwrap();
    }
}
