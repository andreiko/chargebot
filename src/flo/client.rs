use std::time::Duration;

use async_trait::async_trait;
use lazy_static::lazy_static;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use reqwest::header::USER_AGENT;

use crate::utils::error::Error;
use crate::utils::metrics::{common_histogram_buckets, report_api_call_timing, APICallLabels};

use super::models::Park;

/// FLO API client interface.
#[async_trait]
pub trait Client {
    /// Fetches a Park object using its ID.
    async fn get_park(&self, park_id: impl AsRef<str> + Send) -> Result<Park, Error>;
}

/// Implements `Client` by making HTTP requests to the API.
pub struct HTTPClient {
    http_client: reqwest::Client,
    api_origin: String,
    user_agent: Option<String>,
}

const DEFAULT_API_ORIGIN: &str = "https://emobility.flo.ca";

lazy_static! {
    static ref FLO_API_CALLS: Family::<APICallLabels, Histogram> =
        Family::new_with_constructor(|| Histogram::new(common_histogram_buckets()));
}

/// Registers prometheus metrics published by this module.
pub fn register_metrics(reg: &mut Registry) {
    reg.register(
        "flo_api_calls",
        "FLO API call durations",
        FLO_API_CALLS.clone(),
    );
}

impl HTTPClient {
    /// Constructs a new `HTTPClient`.
    pub fn new(timeout: Duration) -> Self {
        Self {
            http_client: reqwest::Client::builder().timeout(timeout).build().unwrap(),
            api_origin: DEFAULT_API_ORIGIN.to_string(),
            user_agent: None,
        }
    }

    /// Replaces the current API origin, allowing to make requests to a dummy server during development.
    pub fn with_api_origin(mut self, api_origin: impl Into<String>) -> Self {
        self.api_origin = api_origin.into();
        self
    }

    /// Replaces the current User-Agent string used by the HTTP client.
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }
}

#[async_trait]
impl Client for HTTPClient {
    async fn get_park(&self, park_id: impl AsRef<str> + Send) -> Result<Park, Error> {
        let park_id = park_id.as_ref();
        report_api_call_timing("get_park", &FLO_API_CALLS, || async {
            let mut req = self
                .http_client
                .get(format!("{}/v3.0/parks/{}/", self.api_origin, park_id));
            if let Some(ua) = &self.user_agent {
                req = req.header(USER_AGENT, ua);
            }

            let resp = req.send().await?;
            Error::expect_status_code(200, &resp)?;

            let park = resp.json().await?;
            log::debug!("pulled latest park info for {}", park_id);
            Ok(park)
        })
        .await
    }
}
