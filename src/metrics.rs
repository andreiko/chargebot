use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use prometheus_client::{encoding::text::encode, registry::Registry};
use tokio::sync::mpsc;
use warp::{http::StatusCode, Filter, Reply};

use crate::utils::filters::match_full_path;

/// Contains configuration options for the metrics exporter.
pub struct Config {
    pub reg: Registry,
    pub bind_addr: SocketAddr,
    pub path: String,
}

/// Warp handler exporting Prometheus metrics from the given registry.
pub async fn metrics_export_endpoint(reg: Arc<Registry>) -> Result<Box<dyn Reply>, Infallible> {
    let mut buf = String::new();
    if encode(&mut buf, &reg).is_err() {
        return Ok::<Box<dyn Reply>, Infallible>(Box::new(StatusCode::INTERNAL_SERVER_ERROR));
    };
    Ok::<Box<dyn Reply>, Infallible>(Box::new(buf))
}

/// Starts the metric exporter server.
///
/// It will shut down when the ` cancel ` channel receives a message or gets closed.
pub async fn start_server(cfg: Config, mut cancel: mpsc::Receiver<()>) {
    log::debug!("starting metrics server");

    let reg = Arc::new(cfg.reg);
    let routes = match_full_path(cfg.path)
        .and(warp::get())
        .and_then(move || {
            let reg = reg.clone();
            metrics_export_endpoint(reg)
        });

    warp::serve(routes)
        .bind_with_graceful_shutdown(cfg.bind_addr, async move {
            cancel.recv().await.unwrap();
        })
        .1
        .await;

    log::debug!("metrics server finished");
}
