use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;

use lazy_static::lazy_static;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;
use tokio::select;
use tokio::sync::mpsc;

use crate::config::Location;
use crate::datasource::Datasource;
use crate::flo::models::{Station, Status};
use crate::flo::watch::Command;
use crate::telegram::models::{
    AnswerCallbackQuery, CallbackQuery, ChatAction, ChatType::Private, EditMessageText,
    EntityType::BotCommand, InlineKeyboardButton, Message, ReplyMarkup::InlineKeyboard,
    SendChatAction, SendMessage, Update,
};
use crate::telegram::output::Payload;

lazy_static! {
    static ref TELEGRAM_UPDATES: Counter::<u64> = Counter::default();
    static ref STATION_UPDATES: Counter::<u64> = Counter::default();
}

/// Registers prometheus metrics published by this module.
pub fn register_metrics(reg: &mut Registry) {
    reg.register(
        "telegram_updates",
        "How many telegram updates were received by the bot",
        TELEGRAM_UPDATES.clone(),
    );
    reg.register(
        "station_updates",
        "How many station updates were received by the bot",
        STATION_UPDATES.clone(),
    );
}

/// Contains configuration options for the bot processor.
pub struct Config {
    pub datasource: Datasource,
    pub telegram_updates: mpsc::Receiver<Update>,
    pub station_updates: mpsc::Receiver<Station>,
    pub telegram_payloads: mpsc::Sender<Payload>,
    pub watch_commands: mpsc::Sender<Command>,
    pub fail: mpsc::Sender<()>,
}

/// Starts the bot processor.
///
/// It listens for updates form Telegram and station watcher, interacts with users over Telegram API
/// and manages station watch subscriptions.
///
/// The processor will shut down when any of the input channels gets closed.
/// Unless it stopped because the closure of `telegram_updates`, it will also send () to the `fail` channel.
pub async fn start(mut cfg: Config) {
    let mut state = State::default();
    let ds = Arc::new(cfg.datasource);

    loop {
        let ds = ds.clone();
        let outputs = select! {
            update_opt = cfg.telegram_updates.recv() => {
                let Some(update) = update_opt else { break };

                TELEGRAM_UPDATES.inc();

                if let Some(message) = update.message {
                    process_message(&message, &mut state, ds.clone())
                } else if let Some(callback_query) = update.callback_query {
                    process_callback_query(&callback_query, &mut state, ds.clone())
                } else {
                    continue;
                }
            }
            station_opt = cfg.station_updates.recv() => {
                let Some(station) = station_opt else {
                    cfg.fail.send(()).await.unwrap();
                    break;
                };

                STATION_UPDATES.inc();

                process_station(&station, &mut state, ds.clone())
            }
        };

        if let Err(err) =
            dispatch_outputs(outputs, &cfg.telegram_payloads, &cfg.watch_commands).await
        {
            // if dispatch fails here, it means that an unrecoverable error happened in
            // a downstream component and the program is shutting down
            cfg.fail.send(()).await.unwrap();
            return log::error!("processor failed: {}", err);
        }
    }

    log::debug!("processor finished");
}

async fn dispatch_outputs(
    outputs: Vec<Output>,
    payloads: &mpsc::Sender<Payload>,
    commands: &mpsc::Sender<Command>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for out in outputs {
        match out {
            Output::TelegramPayload(request) => payloads
                .send(request)
                .await
                .map_err(|_| "couldn't send telegram request")?,
            Output::WatchCommand(command) => commands
                .send(command)
                .await
                .map_err(|_| "couldn't send watch command")?,
        }
    }

    Ok(())
}

/// Represents bot's internal state.
#[derive(Default)]
struct State {
    pub location_chat_subscriptions: HashMap<String, HashSet<i64>>,
    pub station_availability: HashMap<String, bool>,
}

/// Builds notification text given a location and station availability map.
fn prepare_notification_text(
    location: &Location,
    availability: &HashMap<String, bool>,
) -> Option<String> {
    let station_availability = location
        .all_stations()
        .map(|s| (availability.get(&s.id), s.alias.as_str()))
        .collect::<Vec<_>>();
    let station_count = station_availability.len();

    // don't proceed if some statuses are still unknown
    if !station_availability
        .iter()
        .all(|(status, _)| status.is_some())
    {
        return None;
    }

    let available_station_aliases = station_availability
        .iter()
        .flat_map(|(status, alias)| {
            if let Some(true) = status {
                Some(*alias)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    Some(if available_station_aliases.len() == station_count {
        format!(
            "All chargers are available at {}. Should I /stop?",
            location.name
        )
    } else if available_station_aliases.len() > 1 {
        let n = available_station_aliases.len();
        format!(
            "Chargers {} and {} are available at {}. Should I /stop?",
            available_station_aliases[0..n - 1].join(", "),
            available_station_aliases.last().unwrap(),
            location.name
        )
    } else if !available_station_aliases.is_empty() {
        format!(
            "Charger {} is available at {}. Should I /stop?",
            available_station_aliases.first().unwrap(),
            location.name
        )
    } else {
        format!(
            "No chargers at {} can be used. Should I /stop?",
            location.name
        )
    })
}

/// Handles a station status update. If any chats are subscribed to the station's location,
/// updates its stored availability in state and generates notifications for each of the chats.
fn process_station(station: &Station, state: &mut State, ds: Arc<Datasource>) -> Vec<Output> {
    let Some(location) = ds.get_location_by_station(&station.id) else {
        // don't proceed if the station doesn't belong to a known location
        return vec![];
    };

    let Some(chat_ids) = state.location_chat_subscriptions.get(&location.id) else {
        // don't proceed if no chats are subscribed to this station's location
        return vec![];
    };

    if chat_ids.is_empty() {
        return vec![];
    }

    let currently_available = matches!(station.status, Status::Available);
    if let Some(was_available) = state.station_availability.get_mut(&station.id) {
        if *was_available == currently_available {
            // don't proceed if the station's known availability status hasn't changed
            return vec![];
        }
        *was_available = currently_available;
    } else {
        state
            .station_availability
            .insert(station.id.clone(), currently_available);
    }

    let Some(notification_text) = prepare_notification_text(location, &state.station_availability)
    else {
        return vec![];
    };

    chat_ids
        .iter()
        .map(|chat_id| SendMessage::new(*chat_id, notification_text.clone()).into())
        .collect()
}

/// Handles text commands from a Telegram user:
/// /start: Shows an inline keyboard with locations configured for this chat
/// /stop:  Cancels all location subscriptions for the chat.
///         Purges associated internal state for locations that aren't watched by any other chats.
fn process_message(message: &Message, state: &mut State, db: Arc<Datasource>) -> Vec<Output> {
    let Some(entity) = message.entities.first() else {
        return vec![];
    };

    let command = if message.entities.len() == 1
        && matches!(entity.typ, BotCommand)
        && message.text.len() >= entity.offset + entity.length
    {
        (message.text[entity.offset..entity.offset + entity.length]).to_lowercase()
    } else {
        return vec![];
    };

    let chat_locations = db.get_locations_by_chat(message.chat.id);

    if command == "/start" || command.starts_with("/start@") {
        let loc_buttons: Vec<Vec<InlineKeyboardButton>> = chat_locations
            .map(|loc| {
                vec![InlineKeyboardButton {
                    text: loc.name.clone(),
                    callback_data: loc.id.clone(),
                }]
            })
            .collect();

        if loc_buttons.is_empty() {
            return vec![SendMessage::new(
                message.chat.id,
                "Sorry. I'm not configured to work in this chat.",
            )
            .into()];
        }

        return vec![
            SendMessage::new(message.chat.id, "Where do you want to charge?")
                .with_reply_markup(Some(InlineKeyboard(loc_buttons)))
                .into(),
        ];
    } else if command == "/stop" || command.starts_with("/stop@") {
        let mut outputs = Vec::<Output>::new();

        for location in db.get_locations_by_chat(message.chat.id) {
            if let Some(ids) = state.location_chat_subscriptions.get_mut(&location.id) {
                ids.remove(&message.chat.id);
                if ids.is_empty() {
                    // if there are no more chats subscribed to this location, unsubscribe form updates
                    for park in location.parks.iter() {
                        outputs.push(Command::UnsubscribeFromPark(park.id.clone()).into());

                        // invalidate known availability for all of the location's stations
                        for station in park.stations.iter() {
                            state.station_availability.remove(&station.id);
                        }
                    }
                }
            }
        }

        outputs.push(
            SendMessage::new(
                message.chat.id,
                "Won't send any more updates here. Let me know when to /start again.",
            )
            .into(),
        );

        return outputs;
    }

    vec![]
}

/// Handles a click on the inline keyboard when a user subscribes a chat to a location.
/// Creates watcher subscriptions for all parks associated with the location. Modifies the original
/// message to remove the inline keyboard and denote the choice. If another chat is already subscribed
/// to the location and all the statuses are known, sends the response immediately. Otherwise, sends
/// typing notification while the statuses are synced by the watcher.
fn process_callback_query(
    callback_query: &CallbackQuery,
    state: &mut State,
    db: Arc<Datasource>,
) -> Vec<Output> {
    let location = db
        .get_locations_by_chat(callback_query.message.chat.id)
        .find(|loc| loc.id == callback_query.data);

    let location = if let Some(loc) = location {
        loc
    } else {
        return vec![];
    };

    let mut outputs: Vec<Output> = location
        .parks
        .iter()
        .map(|park| Command::SubscribeToPark(park.id.clone()).into())
        .collect();

    outputs.push(AnswerCallbackQuery::new(callback_query.id.clone(), location.name.clone()).into());

    match state.location_chat_subscriptions.get_mut(&location.id) {
        Some(chat_ids) => {
            chat_ids.insert(callback_query.message.chat.id);
        }
        None => {
            state.location_chat_subscriptions.insert(
                location.id.clone(),
                HashSet::from([callback_query.message.chat.id]),
            );
        }
    };

    let new_text = if matches!(callback_query.message.chat.typ, Private) {
        format!("Where do you want to charge?\n👉*{}*", location.name)
    } else {
        format!(
            "Where do you want to charge?\n{}👉*{}*",
            callback_query.from.username, location.name
        )
    };

    outputs.push(
        EditMessageText::new(
            callback_query.message.chat.id,
            callback_query.message.message_id,
            new_text,
        )
        .into(),
    );

    outputs.push(
        if let Some(text) = prepare_notification_text(location, &state.station_availability) {
            // In case another chat is already subscribed to this location and all statuses are known,
            // send the notification immediately.
            SendMessage::new(callback_query.message.chat.id, text).into()
        } else {
            // If station statuses need to be gathered, send a typing notification for now.
            SendChatAction::new(callback_query.message.chat.id, ChatAction::Typing).into()
        },
    );

    outputs
}

/// Types of actions that bot can take in reaction to updates.
pub enum Output {
    TelegramPayload(Payload),
    WatchCommand(Command),
}

impl From<Command> for Output {
    fn from(value: Command) -> Self {
        Output::WatchCommand(value)
    }
}

impl<T: Into<Payload>> From<T> for Output {
    fn from(value: T) -> Self {
        Output::TelegramPayload(value.into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ops::AddAssign;

    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    use crate::config;
    use crate::datasource::Datasource;
    use crate::flo::models::{Station, Status};
    use crate::flo::watch::Command;
    use crate::telegram::models::{
        AnswerCallbackQuery, CallbackQuery, Chat, ChatAction, ChatType, EditMessageText,
        EntityType, Message, MessageEntity, ReplyMarkup, SendChatAction, SendMessage, Update, User,
    };
    use crate::telegram::output::Payload;
    use crate::utils::testing::{expect_no_recv, expect_recv};

    use super::{start, Config};

    struct Counter<T> {
        next: T,
        increment: T,
    }

    impl<T> Counter<T> {
        fn new(first: T, increment: T) -> Self {
            Self {
                next: first,
                increment,
            }
        }
    }

    impl<T: AddAssign + Copy> Iterator for Counter<T> {
        type Item = T;

        fn next(&mut self) -> Option<Self::Item> {
            let result = self.next;
            self.next += self.increment;
            Some(result)
        }
    }

    struct TestEnv {
        serial: Counter<u64>,
        join: JoinHandle<()>,
        updates: mpsc::Sender<Update>,
        stations: mpsc::Sender<Station>,
        payloads: mpsc::Receiver<Payload>,
        commands: mpsc::Receiver<Command>,
        cancel: mpsc::Receiver<()>,
    }

    impl TestEnv {
        async fn send_start(&mut self, chat: Chat) -> Message {
            let update_id = self.serial.next().unwrap();
            let message_id = self.serial.next().unwrap();
            self.send_message(
                update_id,
                Message {
                    message_id,
                    chat,
                    text: "/start".to_string(),
                    entities: vec![MessageEntity {
                        offset: 0,
                        length: 6,
                        typ: EntityType::BotCommand,
                    }],
                },
            )
            .await
        }

        async fn send_stop(&mut self, chat: Chat) -> Message {
            let update_id = self.serial.next().unwrap();
            let message_id = self.serial.next().unwrap();

            self.send_message(
                update_id,
                Message {
                    message_id,
                    chat,
                    text: "/stop".to_string(),
                    entities: vec![MessageEntity {
                        offset: 0,
                        length: 5,
                        typ: EntityType::BotCommand,
                    }],
                },
            )
            .await
        }

        async fn send_callback_query(
            &mut self,
            src_msg: Message,
            data: impl Into<String>,
            username: impl Into<String>,
        ) -> CallbackQuery {
            let update_id = self.serial.next().unwrap();
            let callback_id = self.serial.next().unwrap().to_string();

            let query = CallbackQuery {
                id: callback_id,
                message: src_msg,
                data: data.into(),
                from: User {
                    username: username.into(),
                },
            };

            self.updates
                .send(Update {
                    update_id,
                    message: None,
                    callback_query: Some(query.clone()),
                })
                .await
                .unwrap();

            query
        }

        async fn send_message(&mut self, update_id: u64, msg: Message) -> Message {
            self.updates
                .send(Update {
                    update_id,
                    message: Some(msg.clone()),
                    callback_query: None,
                })
                .await
                .unwrap();

            msg
        }

        async fn send_station(&mut self, id: impl Into<String>, status: Status) -> Station {
            let s = Station {
                id: id.into(),
                status,
            };
            self.stations.send(s.clone()).await.unwrap();

            s
        }

        async fn expect_send_message(&mut self, asserts: impl FnOnce(&SendMessage)) -> SendMessage {
            let Payload::SendMessage(msg) = expect_recv(&mut self.payloads).await.unwrap() else {
                panic!("unexpected payload type");
            };
            asserts(&msg);
            msg
        }

        async fn expect_edit_message_text(
            &mut self,
            asserts: impl FnOnce(&EditMessageText),
        ) -> EditMessageText {
            let Payload::EditMessageText(msg) = expect_recv(&mut self.payloads).await.unwrap()
            else {
                panic!("unexpected payload type");
            };
            asserts(&msg);
            msg
        }

        async fn expect_answer_callback_query(
            &mut self,
            callback_query_id: &str,
            text: &str,
        ) -> AnswerCallbackQuery {
            let Payload::AnswerCallbackQuery(acq) = expect_recv(&mut self.payloads).await.unwrap()
            else {
                panic!("unexpected payload type");
            };
            assert_eq!(acq.callback_query_id, callback_query_id);
            assert_eq!(acq.text, text);
            acq
        }

        async fn expect_typing_chat_action(&mut self, chat_id: i64) -> SendChatAction {
            let sca = match expect_recv(&mut self.payloads).await.unwrap() {
                Payload::SendChatAction(sca) => sca,
                Payload::EditMessageText(_) => {
                    panic!("unexpected payload type: EditMessageText");
                }
                Payload::AnswerCallbackQuery(_) => {
                    panic!("unexpected payload type: AnswerCallbackQuery");
                }
                Payload::SendMessage(sm) => {
                    panic!(
                        "unexpected payload type: {}",
                        serde_json::to_string(&sm).unwrap()
                    );
                }
            };

            assert_eq!(sca.chat_id, chat_id);
            assert!(matches!(sca.action, ChatAction::Typing));
            sca
        }

        async fn expect_subscribe_to_park(&mut self, park_id: &str) {
            let Command::SubscribeToPark(p) = expect_recv(&mut self.commands).await.unwrap() else {
                panic!("unexpected command");
            };
            assert_eq!(p, park_id);
        }

        async fn expect_unsubscribe_from_park(&mut self, park_id: &str) {
            let Command::UnsubscribeFromPark(p) = expect_recv(&mut self.commands).await.unwrap()
            else {
                panic!("unexpected command");
            };
            assert_eq!(p, park_id);
        }

        fn message_from_send_message(&mut self, sm: SendMessage) -> Message {
            Message {
                message_id: self.serial.next().unwrap(),
                chat: if sm.chat_id < 0 {
                    group_chat(sm.chat_id)
                } else {
                    private_chat(sm.chat_id)
                },
                text: sm.text,
                entities: vec![],
            }
        }
    }

    fn private_chat(id: i64) -> Chat {
        Chat {
            id,
            typ: ChatType::Private,
        }
    }

    fn group_chat(id: i64) -> Chat {
        Chat {
            id,
            typ: ChatType::Group,
        }
    }

    fn setup() -> TestEnv {
        let (updates_tx, updates_rx) = mpsc::channel::<Update>(10);
        let (commands_tx, commands_rx) = mpsc::channel::<Command>(10);
        let (stations_tx, stations_rx) = mpsc::channel::<Station>(10);
        let (payloads_tx, payloads_rx) = mpsc::channel::<Payload>(10);
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

        let datasource = Datasource::new(
            vec![
                config::Location {
                    id: "l1".to_string(),
                    name: "L1".to_string(),
                    parks: vec![
                        config::Park {
                            id: "l1p1".to_string(),
                            stations: vec![config::Station {
                                id: "l1p1s1".to_string(),
                                alias: "L1P1S1".to_string(),
                            }],
                        },
                        config::Park {
                            id: "l1p2".to_string(),
                            stations: vec![
                                config::Station {
                                    id: "l1p2s1".to_string(),
                                    alias: "L1P2S1".to_string(),
                                },
                                config::Station {
                                    id: "l1p2s2".to_string(),
                                    alias: "L1P2S2".to_string(),
                                },
                            ],
                        },
                    ],
                },
                config::Location {
                    id: "l2".to_string(),
                    name: "L2".to_string(),
                    parks: vec![config::Park {
                        id: "l2p1".to_string(),
                        stations: vec![config::Station {
                            id: "l2p1s1".to_string(),
                            alias: "L2P1S1".to_string(),
                        }],
                    }],
                },
            ],
            vec![
                config::Chat {
                    id: 123,
                    locations: vec!["l1".to_string()],
                },
                config::Chat {
                    id: -456,
                    locations: vec!["l1".to_string(), "l2".to_string()],
                },
            ],
        )
        .unwrap();

        let join = tokio::spawn(start(Config {
            datasource,
            telegram_updates: updates_rx,
            station_updates: stations_rx,
            telegram_payloads: payloads_tx,
            watch_commands: commands_tx,
            fail: cancel_tx,
        }));

        TestEnv {
            join,
            serial: Counter::<u64>::new(1, 1),
            updates: updates_tx,
            stations: stations_tx,
            payloads: payloads_rx,
            commands: commands_rx,
            cancel: cancel_rx,
        }
    }

    #[tokio::test]
    async fn basic_happy() {
        let mut t = setup();

        t.send_start(private_chat(123)).await;
        let sm = t
            .expect_send_message(|msg| {
                assert_eq!(msg.chat_id, 123);
                assert_eq!(msg.text, "Where do you want to charge?");
                let markup = if let Some(m) = &msg.reply_markup {
                    m
                } else {
                    panic!("message doesn't contain reply markup");
                };

                let ReplyMarkup::InlineKeyboard(kb) = markup;
                assert_eq!(kb.len(), 1);
                assert_eq!(kb[0].len(), 1);
                assert_eq!(kb[0][0].text, "L1");
                assert_eq!(kb[0][0].callback_data, "l1");
            })
            .await;
        let msg1 = t.message_from_send_message(sm);

        let cb = t.send_callback_query(msg1.clone(), "l1", "bob").await;
        t.expect_subscribe_to_park("l1p1").await;
        t.expect_subscribe_to_park("l1p2").await;
        t.expect_answer_callback_query(&cb.id, "L1").await;
        t.expect_edit_message_text(|edit| {
            assert_eq!(edit.message_id, msg1.message_id);
            assert_eq!(edit.chat_id, 123);
            assert_eq!(edit.text, "Where do you want to charge?\n👉*L1*");
        })
        .await;
        t.expect_typing_chat_action(123).await;

        t.send_station("l1p1s1", Status::InUse).await;
        t.send_station("l1p2s1", Status::InUse).await;
        t.send_station("l1p2s2", Status::InUse).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(msg.text, "No chargers at L1 can be used. Should I /stop?");
        })
        .await;

        t.send_station("l1p2s1", Status::Available).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(
                msg.text,
                "Charger L1P2S1 is available at L1. Should I /stop?"
            );
        })
        .await;

        t.send_stop(private_chat(123)).await;
        t.expect_unsubscribe_from_park("l1p1").await;
        t.expect_unsubscribe_from_park("l1p2").await;

        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(
                msg.text,
                "Won't send any more updates here. Let me know when to /start again."
            );
        })
        .await;

        drop(t.updates);
        t.join.await.unwrap();
    }

    #[tokio::test]
    async fn propagates_watcher_failure() {
        let mut t = setup();
        drop(t.stations);

        expect_recv(&mut t.cancel).await.unwrap();
        t.join.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_chat() {
        let mut t = setup();

        t.send_start(private_chat(567)).await;

        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 567);
            assert_eq!(msg.text, "Sorry. I'm not configured to work in this chat.");
        })
        .await;

        drop(t.updates);
        t.join.await.unwrap();
    }

    #[tokio::test]
    async fn parallel_watches() {
        let mut t = setup();

        // chat 1 subscribes to L1
        t.send_start(private_chat(123)).await;
        let sm = t.expect_send_message(|_| {}).await;

        let msg = t.message_from_send_message(sm);
        let cb = t.send_callback_query(msg, "l1", "bob").await;
        t.expect_subscribe_to_park("l1p1").await;
        t.expect_subscribe_to_park("l1p2").await;
        t.expect_answer_callback_query(&cb.id, "L1").await;
        t.expect_edit_message_text(|_| {}).await;
        t.expect_typing_chat_action(123).await;

        // chat 1 gets notified after L1 statuses get synced
        t.send_station("l1p1s1", Status::InUse).await;
        t.send_station("l1p2s1", Status::Available).await;
        t.send_station("l1p2s2", Status::InUse).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(
                msg.text,
                "Charger L1P2S1 is available at L1. Should I /stop?"
            );
        })
        .await;

        // chat 2 subscribes to L2
        t.send_start(group_chat(-456)).await;
        let sm = t.expect_send_message(|_| {}).await;

        let msg = t.message_from_send_message(sm);
        let cb = t.send_callback_query(msg.clone(), "l2", "alice").await;
        t.expect_subscribe_to_park("l2p1").await;
        t.expect_answer_callback_query(&cb.id, "L2").await;
        t.expect_edit_message_text(|edit| {
            assert_eq!(edit.message_id, msg.message_id);
            assert_eq!(edit.chat_id, -456);
            assert_eq!(edit.text, "Where do you want to charge?\nalice👉*L2*");
        })
        .await;
        t.expect_typing_chat_action(-456).await;

        // chat 2 gets notified after L2 status gets synced
        t.send_station("l2p1s1", Status::Available).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, -456);
            assert_eq!(
                msg.text,
                "All chargers are available at L2. Should I /stop?"
            );
        })
        .await;

        // chat 2 subscribes to L1 and gets an immediate notification of already known statuses
        t.send_start(group_chat(-456)).await;
        let sm = t.expect_send_message(|_| {}).await;

        let msg = t.message_from_send_message(sm);
        let cb = t.send_callback_query(msg.clone(), "l1", "alice").await;
        t.expect_subscribe_to_park("l1p1").await;
        t.expect_subscribe_to_park("l1p2").await;
        t.expect_answer_callback_query(&cb.id, "L1").await;
        t.expect_edit_message_text(|_| {}).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, -456);
            assert_eq!(
                msg.text,
                "Charger L1P2S1 is available at L1. Should I /stop?"
            );
        })
        .await;

        // L1 station update notifies both chats
        t.send_station("l1p1s1", Status::Available).await;

        let mut expect_chats = HashSet::<i64>::from([123, -456]);
        t.expect_send_message(|msg| {
            assert!(expect_chats.remove(&msg.chat_id));
            assert_eq!(
                msg.text,
                "Chargers L1P1S1 and L1P2S1 are available at L1. Should I /stop?"
            );
        })
        .await;
        t.expect_send_message(|msg| {
            assert!(expect_chats.remove(&msg.chat_id));
            assert_eq!(
                msg.text,
                "Chargers L1P1S1 and L1P2S1 are available at L1. Should I /stop?"
            );
        })
        .await;
        assert!(expect_chats.is_empty());

        // chat 2 stops all subscriptions
        t.send_stop(group_chat(-456)).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, -456);
            assert_eq!(
                msg.text,
                "Won't send any more updates here. Let me know when to /start again."
            );
        })
        .await;
        t.expect_unsubscribe_from_park("l2p1").await;
        expect_no_recv(&mut t.commands).await;

        // chat 2 doesn't get notified about L2
        t.send_station("l2p1s1", Status::InUse).await;
        expect_no_recv(&mut t.payloads).await;

        // chat 1 still gets notified about L1
        t.send_station("l1p2s2", Status::Available).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(
                msg.text,
                "All chargers are available at L1. Should I /stop?"
            );
        })
        .await;
        expect_no_recv(&mut t.payloads).await;

        // chat 2 subscribes to L2 again
        t.send_start(group_chat(-456)).await;
        let sm = t.expect_send_message(|_| {}).await;

        let msg = t.message_from_send_message(sm);
        let cb = t.send_callback_query(msg.clone(), "l2", "alice").await;
        t.expect_subscribe_to_park("l2p1").await;
        t.expect_answer_callback_query(&cb.id, "L2").await;
        t.expect_edit_message_text(|_| {}).await;
        t.expect_typing_chat_action(-456).await;

        // still only chat 1 gets notified about L1
        t.send_station("l1p2s2", Status::InUse).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, 123);
            assert_eq!(
                msg.text,
                "Chargers L1P1S1 and L1P2S1 are available at L1. Should I /stop?"
            );
        })
        .await;
        expect_no_recv(&mut t.payloads).await;

        // chat 2 gets notified after L2 status gets synced
        t.send_station("l2p1s1", Status::Available).await;
        t.expect_send_message(|msg| {
            assert_eq!(msg.chat_id, -456);
            assert_eq!(
                msg.text,
                "All chargers are available at L2. Should I /stop?"
            );
        })
        .await;

        drop(t.updates);
        t.join.await.unwrap();
    }
}
