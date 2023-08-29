use serde::{Deserialize, Deserializer, Serialize};

/// Minimal structure representing the Update API object:
/// https://core.telegram.org/bots/api#update
#[derive(Serialize, Deserialize, Clone)]
pub struct Update {
    pub update_id: u64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}

impl Update {
    pub fn new(update_id: u64) -> Self {
        Update {
            update_id,
            message: None,
            callback_query: None,
        }
    }
}

/// Minimal structure representing the Message API object:
/// https://core.telegram.org/bots/api#message
#[derive(Serialize, Deserialize, Clone)]
pub struct Message {
    pub message_id: u64,
    pub chat: Chat,
    pub text: String,
    #[serde(default)]
    pub entities: Vec<MessageEntity>,
}

/// Minimal structure representing the MessageEntity API object:
/// https://core.telegram.org/bots/api#messageentity
#[derive(Serialize, Deserialize, Clone)]
pub struct MessageEntity {
    pub offset: usize,
    pub length: usize,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "EntityType::deserialize_fallback"
    )]
    pub typ: EntityType,
}

/// Minimal enum for MessageEntity.type.
#[derive(Serialize, Deserialize, Default, Clone)]
pub enum EntityType {
    #[serde(rename = "bot_command")]
    BotCommand,
    #[default]
    Other,
}

impl EntityType {
    fn deserialize_fallback<'a, D: Deserializer<'a>>(
        deserializer: D,
    ) -> Result<EntityType, D::Error> {
        Ok(EntityType::deserialize(deserializer).unwrap_or(EntityType::default()))
    }
}

/// Minimal structure representing the CallbackQuery API object:
/// https://core.telegram.org/bots/api#callbackquery
#[derive(Serialize, Deserialize, Clone)]
pub struct CallbackQuery {
    pub id: String,
    pub message: Message,
    pub data: String,
    pub from: User,
}

/// Minimal structure representing the User API object:
/// https://core.telegram.org/bots/api#user
#[derive(Serialize, Deserialize, Clone)]
pub struct User {
    pub username: String,
}

/// Minimal structure representing the Chat API object:
/// https://core.telegram.org/bots/api#chat
#[derive(Serialize, Deserialize, Clone)]
pub struct Chat {
    pub id: i64,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "ChatType::deserialize_fallback"
    )]
    pub typ: ChatType,
}

/// Minimal enum for Chat.type.
#[derive(Serialize, Deserialize, Default, Clone)]
pub enum ChatType {
    #[serde(rename = "private")]
    Private,
    #[serde(rename = "group")]
    Group,
    #[default]
    Other,
}

impl ChatType {
    fn deserialize_fallback<'a, D: Deserializer<'a>>(
        deserializer: D,
    ) -> Result<ChatType, D::Error> {
        Ok(ChatType::deserialize(deserializer).unwrap_or(ChatType::default()))
    }
}

/// Represents objects that are sent to the API.
pub trait Payload {
    fn get_method_name() -> &'static str;
}

/// Minimal payload for the sendMessage API method.
/// https://core.telegram.org/bots/api#sendmessage
#[derive(Serialize, Deserialize, Clone)]
pub struct SendMessage {
    pub chat_id: i64,
    pub text: String,
    pub parse_mode: ParseMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<ReplyMarkup>,
}

impl SendMessage {
    pub fn new(chat_id: i64, text: impl Into<String>) -> SendMessage {
        SendMessage {
            chat_id,
            text: text.into(),
            parse_mode: ParseMode::Markdown,
            reply_markup: None,
        }
    }

    pub fn with_reply_markup(mut self, reply_markup: Option<ReplyMarkup>) -> Self {
        self.reply_markup = reply_markup;
        self
    }
}

impl Payload for SendMessage {
    fn get_method_name() -> &'static str {
        "sendMessage"
    }
}

/// Minimal enum for SendMessage.parse_mode. This is always markdown.
#[derive(Serialize, Deserialize, Clone)]
pub enum ParseMode {
    Markdown,
}

/// Minimal structure representing the InlineKeyboardButton API object:
/// https://core.telegram.org/bots/api#inlinekeyboardbutton
#[derive(PartialEq, Serialize, Deserialize, Clone)]
pub struct InlineKeyboardButton {
    pub text: String,
    pub callback_data: String,
}

impl InlineKeyboardButton {
    pub fn new(text: impl Into<String>, callback_data: impl Into<String>) -> InlineKeyboardButton {
        InlineKeyboardButton {
            text: text.into(),
            callback_data: callback_data.into(),
        }
    }
}

/// Minimal enum for SendMessage.reply_markup.
#[derive(Serialize, Deserialize, Clone)]
pub enum ReplyMarkup {
    #[serde(rename = "inline_keyboard")]
    InlineKeyboard(Vec<Vec<InlineKeyboardButton>>),
}

/// Minimal payload for the editMessageText API method.
/// https://core.telegram.org/bots/api#editmessagetext
#[derive(Serialize, Deserialize, Clone)]
pub struct EditMessageText {
    pub chat_id: i64,
    pub message_id: u64,
    pub text: String,
    pub parse_mode: ParseMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<ReplyMarkup>,
}

impl EditMessageText {
    pub fn new(chat_id: i64, message_id: u64, text: impl Into<String>) -> EditMessageText {
        EditMessageText {
            chat_id,
            message_id,
            text: text.into(),
            parse_mode: ParseMode::Markdown,
            reply_markup: None,
        }
    }

    pub fn with_reply_markup(mut self, reply_markup: Option<ReplyMarkup>) -> Self {
        self.reply_markup = reply_markup;
        self
    }
}

impl Payload for EditMessageText {
    fn get_method_name() -> &'static str {
        "editMessageText"
    }
}

/// Minimal payload for the sendChatAction API method.
/// https://core.telegram.org/bots/api#sendchataction
#[derive(Serialize, Deserialize, Clone)]
pub struct SendChatAction {
    pub chat_id: i64,
    pub action: ChatAction,
}

impl SendChatAction {
    pub fn new(chat_id: i64, action: ChatAction) -> SendChatAction {
        SendChatAction { chat_id, action }
    }
}

impl Payload for SendChatAction {
    fn get_method_name() -> &'static str {
        "sendChatAction"
    }
}

/// Minimal enum for SendChatAction.action.
#[derive(Serialize, Deserialize, Clone)]
pub enum ChatAction {
    #[serde(rename = "typing")]
    Typing,
}

/// Minimal payload for the answerCallbackQuery API method.
/// https://core.telegram.org/bots/api#answercallbackquery
#[derive(Serialize, Deserialize, Clone)]
pub struct AnswerCallbackQuery {
    pub callback_query_id: String,
    pub text: String,
}

impl AnswerCallbackQuery {
    pub fn new(
        callback_query_id: impl Into<String>,
        text: impl Into<String>,
    ) -> AnswerCallbackQuery {
        AnswerCallbackQuery {
            callback_query_id: callback_query_id.into(),
            text: text.into(),
        }
    }
}

impl Payload for AnswerCallbackQuery {
    fn get_method_name() -> &'static str {
        "answerCallbackQuery"
    }
}

/// Minimal parameters for the getUpdates API request.
/// https://core.telegram.org/bots/api#getupdates
#[derive(Serialize)]
pub struct GetUpdates {
    pub offset: i64,
    pub timeout: u16,
}
