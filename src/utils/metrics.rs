use std::future::Future;

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use tokio::time::Instant;

use super::error::Error;

// Prometheus labels struct for metrics produced by `report_api_call_timing`.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct APICallLabels {
    pub endpoint: &'static str,
    pub status_code: u16,
    pub outcome: APICallOutcome,
}

// Prometheus label value enum for APICallLabels.outcome.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum APICallOutcome {
    Success,
    Failure,
}

/// Returns timing buckets commonly used by histogram metrics in this app.
pub fn common_histogram_buckets() -> impl Iterator<Item = f64> {
    exponential_buckets(0.005, 2.0, 10)
}

/// Measures timing of a function that calls a single API endpoint and reports it using
/// the provided prometheus metric.
/// The measured function must return the result of the call or the standard `utils::Error`.
pub async fn report_api_call_timing<R, F: Future<Output = Result<R, Error>>>(
    endpoint: &'static str,
    metric: &Family<APICallLabels, Histogram>,
    call: impl FnOnce() -> F,
) -> Result<R, Error> {
    let ts_before_send = Instant::now();
    let (result, labels) = match call().await {
        Ok(park) => (
            Ok(park),
            APICallLabels {
                endpoint,
                status_code: 200,
                outcome: APICallOutcome::Success,
            },
        ),
        Err(err) => {
            let labels = APICallLabels {
                endpoint,
                status_code: match &err {
                    Error::InvalidContents(_) => 200,
                    Error::UnexpectedStatus(code) => *code,
                    _ => 0,
                },
                outcome: APICallOutcome::Failure,
            };
            (Err(err), labels)
        }
    };

    metric
        .get_or_create(&labels)
        .observe(ts_before_send.elapsed().as_secs_f64());

    result
}
