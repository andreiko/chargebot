use std::{
    ffi::OsString, fs::File, io::Read, net::SocketAddr, ops::RangeInclusive, time::Duration,
};

use clap::{Arg, ArgAction, Command};
use serde::{Deserialize, Serialize};
use url::Url;
use validator::{Validate, ValidationError};

use crate::whatever::{SnafuExt, WhateverSync};

/// Top-level configuration struct.
#[derive(Deserialize, Validate)]
pub struct BotConfig {
    /// Specifies locations that can be monitored by the bot.
    #[validate(nested)]
    pub locations: Vec<Location>,

    /// Specifies Telegram chats that can be used to interact with the bot.
    #[validate(nested)]
    pub chats: Vec<Chat>,

    /// Defines settings related to interactions with Telegram API.
    #[validate(nested)]
    pub telegram: Telegram,

    /// Defines settings related to interactions with FLO API.
    #[validate(nested)]
    #[serde(default)]
    pub flo: FLO,

    /// If specified, metrics server will be listening on a dedicated address.
    #[validate(nested)]
    pub metrics_server: Option<MetricsServer>,
}

impl BotConfig {
    /// Creates BotConfig from a reader providing YAML.
    pub fn from_reader(r: impl Read) -> Result<Self, WhateverSync> {
        let cfg = serde_yaml::from_reader::<_, Self>(r).whatever()?;
        cfg.validate().whatever()?;
        Ok(cfg)
    }

    /// Creates BotConfig command line arguments.
    pub fn from_cli_args<I, T>(args: I) -> Result<Self, WhateverSync>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let app = Command::new("chargebot")
            .arg(
                Arg::new("config")
                    .action(ArgAction::Set)
                    .long("config")
                    .short('c')
                    .required(true)
                    .value_name("FILENAME")
                    .help("path to the yaml config file"),
            )
            .get_matches_from(args);

        let filename = app.get_one::<String>("config").unwrap();

        Self::from_reader(File::open(filename).whatever()?)
            .whatever_msg("unable to load config file")
    }
}

/// Defines a named logical location of EV charging Parks.
#[derive(Serialize, Deserialize, Validate)]
pub struct Location {
    // User-defined location ID.
    #[validate(length(min = 1))]
    pub id: String,

    // User-defined location name.
    #[validate(length(min = 1))]
    pub name: String,

    // List of parks at this location.
    #[validate(length(min = 1))]
    pub parks: Vec<Park>,
}

impl Location {
    /// Shortcut for getting a list of all Stations at this location.
    pub fn all_stations(&self) -> impl Iterator<Item = &Station> {
        self.parks.iter().flat_map(|p| &p.stations)
    }
}

/// Defines a group of EV charging stations.
#[derive(Serialize, Deserialize, Validate)]
pub struct Park {
    /// Park ID in FLO API.
    #[validate(length(equal = 36))]
    pub id: String,

    // List of charging stations at this park.
    #[validate(length(min = 1))]
    pub stations: Vec<Station>,
}

/// Defines an EV charging station.
#[derive(Serialize, Deserialize, Validate)]
pub struct Station {
    /// Station ID in FLO API.
    #[validate(length(equal = 36))]
    pub id: String,

    /// User-defined alias for the station, e.g. "Back" or "Right".
    #[validate(length(min = 1))]
    pub alias: String,
}

/// Defines a telegram chat.
#[derive(Serialize, Deserialize, Validate)]
pub struct Chat {
    /// Chat ID in Telegram API.
    #[validate(custom(function = "Chat::validate_id"))]
    pub id: i64,

    /// List of locations accessible from this chat.
    #[validate(length(min = 1))]
    pub locations: Vec<String>,
}

impl Chat {
    /// Validates that the integer is not equal to zero. IDs for group chats are negative.
    fn validate_id(n: i64) -> Result<(), ValidationError> {
        if n != 0 {
            Ok(())
        } else {
            Err(ValidationError::new("chat id must not be equal to zero"))
        }
    }
}

/// Defines settings related to interactions with Telegram API.
#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Validate)]
pub struct Telegram {
    /// A secret for interacting with Telegram API.
    #[validate(length(min = 37))]
    pub api_token: String,

    /// Timeout for requests to Telegram API (except for long polling).
    #[validate(custom(function = "validate_duration"))]
    #[serde(
        rename = "http_timeout_seconds",
        default = "Telegram::default_http_timeout"
    )]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    pub http_timeout: Duration,

    /// Output buffer for payloads to be delivered to Telegram API.
    #[validate(range(min = 1))]
    #[serde(default = "Telegram::default_output_buffer_size")]
    pub output_buffer_size: usize,

    /// If specified, the bot will listen for webhooks to receive updates from Telegram API.
    #[validate(nested)]
    pub webhook: Option<Webhook>,

    /// If specified, the bot will perform long-polling to receive updates from Telegram API.
    #[validate(nested)]
    pub polling: Option<Polling>,

    /// Input buffer for updates to be processed by the bot.
    #[validate(range(min = 1))]
    #[serde(default = "Telegram::default_updates_buffer_size")]
    pub updates_buffer_size: usize,
}

impl Telegram {
    fn default_http_timeout() -> Duration {
        Duration::from_secs(5)
    }
    fn default_output_buffer_size() -> usize {
        100
    }
    fn default_updates_buffer_size() -> usize {
        100
    }
}

/// Defines settings related to webhook server listening for updates from Telegram API.
#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Validate)]
pub struct Webhook {
    /// Server will be listening for updates on this address.
    #[serde(default = "Webhook::default_bind_addr")]
    pub bind_addr: SocketAddr,

    /// Server will be expecting this path to be used by Telegram API when delivering updates.
    #[validate(length(min = 1))]
    pub path: String,

    /// Server will expect webhook requests to contain this token in the special auth header.
    #[validate(length(min = 1))]
    pub secret_token: String,

    /// Timeout for adding the received update to the input buffer.
    #[validate(custom(function = "validate_duration"))]
    #[serde(
        rename = "accept_timeout_seconds",
        default = "Webhook::default_accept_timeout"
    )]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    pub accept_timeout: Duration,

    /// If specified, the webhook server will also export Prometheus metrics at this path.
    #[validate(length(min = 1))]
    pub metrics_path: Option<String>,
}

impl Webhook {
    fn default_bind_addr() -> SocketAddr {
        "0.0.0.0:8000".parse().unwrap()
    }

    fn default_accept_timeout() -> Duration {
        Duration::from_secs(5)
    }
}

/// Defines settings related to long-polling updates from Telegram API.
#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Validate)]
pub struct Polling {
    /// After this duration the polling endpoint will time out to an empty successful response.
    #[validate(range(min = 1))]
    #[serde(default = "Polling::default_duration")]
    pub duration_seconds: u16,

    /// Timeout for long polling requests. Must be greater than `duration_seconds`.
    #[validate(custom(function = "validate_duration"))]
    #[serde(
        rename = "http_timeout_seconds",
        default = "Polling::default_http_timeout"
    )]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    pub http_timeout: Duration,
}

impl Polling {
    fn default_duration() -> u16 {
        60
    }
    fn default_http_timeout() -> Duration {
        Duration::from_secs(65)
    }
}

/// Defines settings related to interactions with FLO API.
#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Validate)]
pub struct FLO {
    /// Timeout for requests to FLO API.
    #[validate(custom(function = "validate_duration"))]
    #[serde(rename = "http_timeout_seconds", default = "FLO::default_http_timeout")]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    pub http_timeout: Duration,

    /// If specified, the bot will send requests to the specified HTTP origin. Convenient for
    /// using a mocked API server during development.
    #[validate(custom(function = "FLO::validate_api_origin"))]
    pub api_origin: Option<String>,

    /// Range for random intervals between calls to FLO API.
    #[validate(nested)]
    #[validate(custom(function = "FLO::validate_poll_interval"))]
    #[serde(
        rename = "poll_interval_seconds",
        default = "FLO::default_poll_interval"
    )]
    pub poll_interval: DurationMinMax,

    /// If specified, the bot will put this string into "User-Agent" header for calls to FLO API.
    #[validate(length(min = 1))]
    pub user_agent: Option<String>,

    /// Buffer size for park subscription requests.
    #[validate(range(min = 1))]
    #[serde(default = "FLO::default_input_buffer_size")]
    pub input_buffer_size: usize,

    /// Buffer size for station status changes to be processed by the bot.
    #[validate(range(min = 1))]
    #[serde(default = "FLO::default_output_buffer_size")]
    pub output_buffer_size: usize,
}

impl FLO {
    fn default_http_timeout() -> Duration {
        Duration::from_secs(5)
    }
    fn default_poll_interval() -> DurationMinMax {
        DurationMinMax {
            min: Duration::from_secs(40),
            max: Duration::from_secs(100),
        }
    }
    fn default_input_buffer_size() -> usize {
        100
    }
    fn default_output_buffer_size() -> usize {
        100
    }
    fn validate_api_origin(origin: &str) -> Result<(), ValidationError> {
        match origin.parse::<Url>() {
            Ok(u) => {
                if u.scheme() != "https" && u.scheme() != "http" {
                    Err(ValidationError::new("url_invalid_protocol"))
                } else if u.path() != "/" || u.query().is_some() || u.fragment().is_some() {
                    Err(ValidationError::new("url_too_specific"))
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(ValidationError::new("url_invalid")),
        }
    }
    fn validate_poll_interval(range: &DurationMinMax) -> Result<(), ValidationError> {
        if range.max >= range.min {
            Ok(())
        } else {
            let mut e = ValidationError::new("invalid_range");
            e.message = Some("max must be greater or equal than min".into());
            Err(e)
        }
    }
}

/// Defines a range of durations.
#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Validate)]
pub struct DurationMinMax {
    #[validate(custom(function = "validate_duration"))]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    min: Duration,
    #[validate(custom(function = "validate_duration"))]
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    max: Duration,
}

impl From<DurationMinMax> for RangeInclusive<Duration> {
    fn from(r: DurationMinMax) -> Self {
        r.min..=r.max
    }
}

impl Default for FLO {
    fn default() -> Self {
        Self {
            http_timeout: FLO::default_http_timeout(),
            api_origin: None,
            poll_interval: FLO::default_poll_interval(),
            user_agent: None,
            input_buffer_size: FLO::default_input_buffer_size(),
            output_buffer_size: FLO::default_output_buffer_size(),
        }
    }
}

/// Defines settings related to prometheus metrics exporter.
#[derive(Serialize, Deserialize, Validate)]
pub struct MetricsServer {
    /// Metrics server will listen on this address.
    #[serde(default = "MetricsServer::default_bind_addr")]
    pub bind_addr: SocketAddr,

    /// Metrics server will expect incoming requests to have this path.
    #[validate(length(min = 1))]
    #[serde(default = "MetricsServer::default_path")]
    pub path: String,
}

impl MetricsServer {
    fn default_bind_addr() -> SocketAddr {
        "0.0.0.0:9090".parse().unwrap()
    }
    fn default_path() -> String {
        "/metrics/".into()
    }
}

/// Validates that the provided duration is greater than zero.
fn validate_duration(d: &Duration) -> Result<(), ValidationError> {
    if !d.is_zero() {
        Ok(())
    } else {
        Err(ValidationError::new("duration must be greater than zero"))
    }
}
