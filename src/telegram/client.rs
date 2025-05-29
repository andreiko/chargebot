use std::{sync::LazyLock, time::Duration};

use async_trait::async_trait;
use prometheus_client::{
    metrics::{family::Family, histogram::Histogram},
    registry::Registry,
};
use serde::{Deserialize, Serialize};

use super::models::{GetUpdates, Payload, Update};
use crate::utils::{
    error::Error,
    metrics::{common_histogram_buckets, report_api_call_timing, APICallLabels},
};

static TELEGRAM_API_CALLS: LazyLock<Family<APICallLabels, Histogram>> =
    LazyLock::new(|| Family::new_with_constructor(|| Histogram::new(common_histogram_buckets())));

/// Registers prometheus metrics published by this module.
pub fn register_metrics(reg: &mut Registry) {
    reg.register(
        "telegram_api_calls",
        "Telegram API call durations",
        TELEGRAM_API_CALLS.clone(),
    );
}

/// Telegram API client interface.
#[async_trait]
pub trait Client {
    async fn deliver_payload<P: Payload + Serialize + Sync>(
        &self,
        payload: &P,
    ) -> Result<(), Error>;
    async fn get_updates(&self, get_updates: GetUpdates) -> Result<Vec<Update>, Error>;
}

/// Implements `Client` by making HTTP requests to the API.
pub struct HTTPClient {
    auth_token: String,
    http_client: reqwest::Client,
}

/// A struct in which API responses are wrapped.
#[derive(Deserialize)]
struct ResponseEnvelope<T> {
    ok: bool,
    result: T,
}

impl HTTPClient {
    pub fn new(auth_token: impl Into<String>, timeout: Duration) -> Self {
        Self {
            auth_token: auth_token.into(),
            http_client: reqwest::Client::builder().timeout(timeout).build().unwrap(),
        }
    }
    fn build_endpoint(auth_token: &str, method_name: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", auth_token, method_name)
    }
}

#[async_trait]
impl Client for HTTPClient {
    async fn deliver_payload<P: Payload + Serialize + Sync>(
        &self,
        payload: &P,
    ) -> Result<(), Error> {
        let method_name = P::get_method_name();
        report_api_call_timing(method_name, &TELEGRAM_API_CALLS, || async {
            let url = Self::build_endpoint(&self.auth_token, method_name);
            let resp = self.http_client.post(url).json(payload).send().await?;
            Error::expect_status_code(200, &resp)?;
            Ok(())
        })
        .await
    }

    async fn get_updates(&self, get_updates: GetUpdates) -> Result<Vec<Update>, Error> {
        report_api_call_timing("get_updates", &TELEGRAM_API_CALLS, || async {
            let url = Self::build_endpoint(&self.auth_token, "getUpdates");
            let resp = self
                .http_client
                .post(url)
                .query(&get_updates)
                .send()
                .await?;
            Error::expect_status_code(200, &resp)?;

            let envelope = resp.json::<ResponseEnvelope<Vec<Update>>>().await?;
            if !envelope.ok {
                return Err(Error::InvalidContents {
                    msg: "ok=false in API response".into(),
                });
            }

            Ok(envelope.result)
        })
        .await
    }
}
