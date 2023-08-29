use backoff::backoff::Backoff;
use tokio::sync::mpsc;

use crate::utils::retry::retry;

use super::client::Client;
use super::models::{AnswerCallbackQuery, EditMessageText, SendChatAction, SendMessage};

/// Contains configuration options for the output writer.
pub struct Config<B: Backoff> {
    pub input: mpsc::Receiver<Payload>,
    pub fail: mpsc::Sender<()>,
    pub backoff: B,
}

/// Types of payloads.
pub enum Payload {
    SendMessage(SendMessage),
    EditMessageText(EditMessageText),
    SendChatAction(SendChatAction),
    AnswerCallbackQuery(AnswerCallbackQuery),
}

impl From<SendMessage> for Payload {
    fn from(value: SendMessage) -> Self {
        Payload::SendMessage(value)
    }
}

impl From<EditMessageText> for Payload {
    fn from(value: EditMessageText) -> Self {
        Payload::EditMessageText(value)
    }
}

impl From<SendChatAction> for Payload {
    fn from(value: SendChatAction) -> Self {
        Payload::SendChatAction(value)
    }
}

impl From<AnswerCallbackQuery> for Payload {
    fn from(value: AnswerCallbackQuery) -> Self {
        Payload::AnswerCallbackQuery(value)
    }
}

/// Starts the output writer.
///
/// It listens to the input channel and delivers the received payloads to Telegram API using
/// the provided `client`.
///
/// The writer will shut down when the `input` channel is closed or when it encounters
/// an unrecoverable error. In the latter case, it will also send () to the `fail` channel.
pub async fn start<B: Backoff>(client: impl Client, mut cfg: Config<B>) {
    let mut input = cfg.input;
    while let Some(req) = input.recv().await {
        let result = retry(
            &mut cfg.backoff,
            || async {
                match &req {
                    Payload::SendMessage(payload) => client.deliver_payload(payload).await,
                    Payload::EditMessageText(payload) => client.deliver_payload(payload).await,
                    Payload::SendChatAction(payload) => client.deliver_payload(payload).await,
                    Payload::AnswerCallbackQuery(payload) => client.deliver_payload(payload).await,
                }
            },
            |e| e.is_transport() || e.is_server(),
            |e| {
                log::error!(
                    "transient error while calling Telegram API (will be retried): {}",
                    e
                )
            },
        )
        .await;

        if let Err(err) = result {
            cfg.fail.send(()).await.unwrap();
            return log::error!("output queue failed: {}", err);
        }
    }

    log::debug!("output writer finished");
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use backoff::backoff::Backoff;
    use serde::Serialize;
    use tokio::sync::mpsc;

    use crate::utils::error::Error;
    use crate::utils::testing::expect_recv;

    use super::super::client::Client;
    use super::super::models::{
        self, AnswerCallbackQuery, ChatAction, EditMessageText, GetUpdates, SendChatAction,
        SendMessage, Update,
    };
    use super::{start, Config, Payload};

    struct MockClient {
        debug_out: mpsc::Sender<Payload>,
        fail_once: Mutex<Option<Error>>,
    }

    impl MockClient {
        fn new(debug_out: mpsc::Sender<Payload>, fail_once: Option<Error>) -> Self {
            Self {
                debug_out,
                fail_once: Mutex::new(fail_once),
            }
        }
    }

    #[async_trait]
    impl Client for MockClient {
        async fn deliver_payload<P: models::Payload + Serialize + Sync>(
            &self,
            payload: &P,
        ) -> Result<(), Error> {
            if let Some(err) = self.fail_once.lock().unwrap().take() {
                return Err(err);
            }

            let dump = serde_json::to_string(payload).unwrap();

            let method = P::get_method_name();
            let p: Payload = match method {
                "sendMessage" => serde_json::from_str::<SendMessage>(&dump).unwrap().into(),
                "editMessageText" => serde_json::from_str::<EditMessageText>(&dump)
                    .unwrap()
                    .into(),
                "answerCallbackQuery" => serde_json::from_str::<AnswerCallbackQuery>(&dump)
                    .unwrap()
                    .into(),
                "sendChatAction" => serde_json::from_str::<SendChatAction>(&dump)
                    .unwrap()
                    .into(),
                _ => {
                    panic!("unexpected payload type: {}", method);
                }
            };

            self.debug_out.send(p).await.unwrap();
            Ok(())
        }

        async fn get_updates(&self, _: GetUpdates) -> Result<Vec<Update>, Error> {
            panic!("unexpected call to get_updates()")
        }
    }

    fn shorter_backoff() -> impl Backoff {
        backoff::ExponentialBackoffBuilder::default()
            .with_initial_interval(Duration::from_millis(5))
            .build()
    }

    #[tokio::test]
    async fn happy_flow() {
        let (payloads_tx, payloads_rx) = mpsc::channel::<Payload>(10);
        let (debug_tx, mut debug_rx) = mpsc::channel::<Payload>(10);
        let (fail_tx, _) = mpsc::channel::<()>(1);
        let telegram_client = MockClient::new(debug_tx, None);

        // start the output writer
        let handle = tokio::spawn(start(
            telegram_client,
            Config {
                input: payloads_rx,
                backoff: shorter_backoff(),
                fail: fail_tx,
            },
        ));

        // send a payload to the channel and expect it back on the mock client debug channel
        payloads_tx
            .send(SendChatAction::new(123, ChatAction::Typing).into())
            .await
            .unwrap();
        if let Payload::SendChatAction(action) = expect_recv(&mut debug_rx).await.unwrap() {
            assert_eq!(action.chat_id, 123);
            assert!(matches!(action.action, ChatAction::Typing));
        } else {
            panic!("unexpected payload variant");
        }

        drop(payloads_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn notifies_of_permanent_failure() {
        let (payloads_tx, payloads_rx) = mpsc::channel::<Payload>(10);
        let (debug_tx, _) = mpsc::channel::<Payload>(10);
        let (fail_tx, mut fail_rx) = mpsc::channel::<()>(1);
        let telegram_client = MockClient::new(debug_tx, Some(Error::UnexpectedStatus(400)));

        // start the output queue processor
        let handle = tokio::spawn(start(
            telegram_client,
            Config {
                input: payloads_rx,
                backoff: shorter_backoff(),
                fail: fail_tx,
            },
        ));

        // mock client will return an unrecoverable error
        payloads_tx
            .send(SendChatAction::new(123, ChatAction::Typing).into())
            .await
            .unwrap();

        // make sure failure channel is notified
        expect_recv(&mut fail_rx).await.unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn retries_transient_errors() {
        let (payloads_tx, payloads_rx) = mpsc::channel::<Payload>(10);
        let (debug_tx, mut debug_rx) = mpsc::channel::<Payload>(10);
        let (fail_tx, _) = mpsc::channel::<()>(1);
        let telegram_client = MockClient::new(debug_tx, Some(Error::UnexpectedStatus(503)));

        let handle = tokio::spawn(start(
            telegram_client,
            Config {
                input: payloads_rx,
                backoff: shorter_backoff(),
                fail: fail_tx,
            },
        ));

        // mock client will return a transient error once, then will accept the payload
        payloads_tx
            .send(SendChatAction::new(123, ChatAction::Typing).into())
            .await
            .unwrap();

        // expect the payload back on the mock client debug channel
        if let Payload::SendChatAction(action) = expect_recv(&mut debug_rx).await.unwrap() {
            assert_eq!(action.chat_id, 123);
            assert!(matches!(action.action, ChatAction::Typing));
        } else {
            panic!("unexpected payload variant");
        }

        drop(payloads_tx);
        handle.await.unwrap();
    }
}
