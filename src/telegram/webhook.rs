use std::{
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use prometheus_client::{
    encoding::{EncodeLabelSet, EncodeLabelValue},
    metrics::{counter::Counter, family::Family},
    registry::Registry,
};
use tokio::sync::{mpsc, mpsc::error::SendTimeoutError};
use warp::{http::StatusCode, reject, Filter, Rejection};

use super::models::Update;
use crate::{metrics::metrics_export_endpoint, utils::filters::match_full_path};

const AUTH_HEADER: &str = "X-Telegram-Bot-Api-Secret-Token";

static TELEGRAM_WEBHOOKS: LazyLock<Family<WebhookLabels, Counter>> = LazyLock::new(Family::default);

// Prometheus labels struct for `TELEGRAM_WEBHOOKS`.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct WebhookLabels {
    pub outcome: WebhookOutcome,
}

// Prometheus label value enum for WebhookLabels.outcome.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum WebhookOutcome {
    Success,
    NotAuthenticated,
    Timeout,
    ChannelClosed,
}

/// Registers prometheus metrics published by this module.
pub fn register_metrics(reg: &mut Registry) {
    reg.register(
        "telegram_webhooks",
        "How many Telegram webhooks requests were received",
        TELEGRAM_WEBHOOKS.clone(),
    );
}

/// Contains configuration options for the webhook.
pub struct Config {
    pub path: String,
    pub secret_token: String,
    pub bind_addr: SocketAddr,
    pub output: mpsc::Sender<Update>,
    pub accept_timeout: Duration,
    pub metrics: Option<MetricsConfig>,
    pub cancel: mpsc::Receiver<()>,
}

/// Contains configuration options for the metrics endpoint served by the webhook server.
pub struct MetricsConfig {
    pub reg: Registry,
    pub path: String,
}

/// Creates a conditional Warp filter for the metrics endpoint.
/// If cfg is Some, the endpoint serves metrics from the provided registry, otherwise returns 404.
fn metrics_optional_filter(
    cfg: Option<MetricsConfig>,
) -> impl Filter<Extract = (Arc<Registry>,), Error = Rejection> + Clone {
    match cfg {
        Some(cfg) => {
            let reg = Arc::new(cfg.reg);
            match_full_path(cfg.path)
                .and(warp::get())
                .and_then(move || {
                    let reg = reg.clone();
                    async move { Ok::<_, Rejection>(reg) }
                })
                .boxed()
        }
        None => warp::any()
            .and_then(|| async { Err::<Arc<Registry>, _>(reject::not_found()) })
            .boxed(),
    }
}

/// Starts the webhook server.
///
/// The server will shut down when the `cancel` channel receives a message or gets closed.
pub fn start(cfg: Config) -> impl Future<Output = ()> {
    let expect_token = Arc::new(cfg.secret_token);

    let webhook_route = match_full_path(cfg.path)
        .and(warp::header(AUTH_HEADER))
        .and(warp::post())
        .and(warp::body::json())
        .and_then(move |token: String, upd: Update| {
            let output = cfg.output.clone();
            let expect_token = expect_token.clone();
            async move {
                if token.as_str() != expect_token.as_str() {
                    TELEGRAM_WEBHOOKS
                        .get_or_create(&WebhookLabels {
                            outcome: WebhookOutcome::NotAuthenticated,
                        })
                        .inc();
                    return Ok::<StatusCode, Infallible>(StatusCode::UNAUTHORIZED);
                }

                if let Err(err) = output.send_timeout(upd, cfg.accept_timeout).await {
                    return match err {
                        SendTimeoutError::Timeout(_) => {
                            TELEGRAM_WEBHOOKS
                                .get_or_create(&WebhookLabels {
                                    outcome: WebhookOutcome::Timeout,
                                })
                                .inc();
                            log::error!("update rejected due to full channel");
                            Ok(StatusCode::SERVICE_UNAVAILABLE)
                        }
                        SendTimeoutError::Closed(_) => {
                            TELEGRAM_WEBHOOKS
                                .get_or_create(&WebhookLabels {
                                    outcome: WebhookOutcome::ChannelClosed,
                                })
                                .inc();
                            log::error!("update rejected due to closed channel");
                            Ok(StatusCode::INTERNAL_SERVER_ERROR)
                        }
                    };
                }

                TELEGRAM_WEBHOOKS
                    .get_or_create(&WebhookLabels {
                        outcome: WebhookOutcome::Success,
                    })
                    .inc();
                Ok(StatusCode::OK)
            }
        });

    let metrics_route = metrics_optional_filter(cfg.metrics)
        .and_then(move |reg: Arc<Registry>| metrics_export_endpoint(reg.clone()));

    let routes = metrics_route.or(webhook_route);

    let mut cancel = cfg.cancel;
    warp::serve(routes)
        .bind_with_graceful_shutdown(cfg.bind_addr, async move {
            cancel.recv().await;
        })
        .1
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{net::TcpSocket, sync::mpsc};

    use super::{super::models::Update, start, Config};
    use crate::utils::testing::{expect_no_recv, expect_recv};

    #[tokio::test]
    async fn basic() {
        // detect an available local port to run the test server on
        let test_addr = {
            let s = TcpSocket::new_v4().unwrap();
            s.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            s.local_addr().unwrap()
        };

        let (updates_tx, mut updates_rx) = mpsc::channel::<Update>(10);
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

        // start the test server
        let handle = tokio::spawn(start(Config {
            path: "/test/webhook/".to_string(),
            secret_token: "abcdef".to_string(),
            bind_addr: test_addr,
            output: updates_tx,
            accept_timeout: Duration::from_secs(5),
            metrics: None,
            cancel: cancel_rx,
        }));

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let test_endpoint = format!("http://{}/test/webhook/", &test_addr);

        // send an update with no auth header, expect 400 status code and no updates on the channel
        let send_update = Update::new(1234);
        let resp = client
            .post(&test_endpoint)
            .json(&send_update)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
        expect_no_recv(&mut updates_rx).await;

        // send an update with a wrong auth header, expect 401 status code and no updates on the channel
        let resp = client
            .post(&test_endpoint)
            .header("X-Telegram-Bot-Api-Secret-Token", "wrong")
            .json(&send_update)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401);
        expect_no_recv(&mut updates_rx).await;

        // send an update with valid auth header, expect 200 status code and an update on the channel
        let resp = client
            .post(&test_endpoint)
            .header("X-Telegram-Bot-Api-Secret-Token", "abcdef")
            .json(&send_update)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let update = expect_recv(&mut updates_rx).await.unwrap();
        assert_eq!(update.update_id, 1234);

        // no trailing updates
        expect_no_recv(&mut updates_rx).await;

        // close the cancellation channel and expect the server to finish
        drop(cancel_tx);
        handle.await.unwrap();
    }
}
