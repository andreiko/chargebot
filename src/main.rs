use std::env::args_os;
use std::process::exit;

use prometheus_client::registry::Registry;
use tokio::sync::mpsc;

use crate::config::BotConfig;
use crate::datasource::Datasource;
use crate::flo::client::HTTPClient as FLOClient;
use crate::flo::models::Station;
use crate::telegram::client::HTTPClient as TelegramClient;
use crate::telegram::models::Update;
use crate::telegram::webhook;
use crate::utils::retry::exp_backoff_forever;

pub mod bot;
pub mod config;
pub mod datasource;
pub mod flo;
pub mod metrics;
pub mod telegram;
pub mod utils;

// TODO: add example config
// TODO: automated push to dockerhub
// TODO: helm chart

#[tokio::main]
async fn main() {
    // init logging
    env_logger::init();

    // init metrics
    let mut prom_registry = Registry::with_prefix("chargebot");
    flo::client::register_metrics(&mut prom_registry);
    telegram::client::register_metrics(&mut prom_registry);
    webhook::register_metrics(&mut prom_registry);
    bot::register_metrics(&mut prom_registry);

    // build configuration
    let bot_cfg = match BotConfig::from_cli_args(args_os()) {
        Ok(cfg) => cfg,
        Err(err) => {
            log::error!("unable to load configuration: {}", err);
            exit(2);
        }
    };

    // init FLO client
    let mut flo_client = FLOClient::new(bot_cfg.flo.http_timeout);
    if let Some(api_origin) = bot_cfg.flo.api_origin {
        flo_client = flo_client.with_api_origin(api_origin);
    }
    if let Some(ua) = bot_cfg.flo.user_agent {
        flo_client = flo_client.with_user_agent(ua);
    }

    // start park watch manager
    let (cmd_tx, cmd_rx) = mpsc::channel::<flo::watch::Command>(bot_cfg.flo.input_buffer_size);
    let (stations_tx, stations_rx) = mpsc::channel::<Station>(bot_cfg.flo.output_buffer_size);
    let watcher_join = tokio::spawn(flo::watch::start(
        flo_client,
        flo::watch::Config {
            input: cmd_rx,
            output: stations_tx,
            poll_interval: bot_cfg.flo.poll_interval.into(),
        },
    ));

    // init cancellation channel
    let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
    tokio::spawn({
        let cancel_tx = cancel_tx.clone();
        async move {
            tokio::signal::ctrl_c().await.unwrap();
            cancel_tx.send(()).await.unwrap();
        }
    });

    // start output writer
    let (payloads_tx, payloads_rx) =
        mpsc::channel::<telegram::output::Payload>(bot_cfg.telegram.output_buffer_size);
    let telegram_client = TelegramClient::new(
        bot_cfg.telegram.api_token.clone(),
        bot_cfg.telegram.http_timeout,
    );
    let output_join = tokio::spawn(telegram::output::start(
        telegram_client,
        telegram::output::Config {
            input: payloads_rx,
            fail: cancel_tx.clone(),
            backoff: exp_backoff_forever(),
        },
    ));

    // start the bot
    let (updates_tx, updates_rx) = mpsc::channel::<Update>(bot_cfg.telegram.updates_buffer_size);
    let bot_join = tokio::spawn(bot::start(bot::Config {
        datasource: Datasource::new(bot_cfg.locations, bot_cfg.chats).unwrap(),
        telegram_updates: updates_rx,
        station_updates: stations_rx,
        telegram_payloads: payloads_tx,
        watch_commands: cmd_tx,
        fail: cancel_tx.clone(),
    }));

    // determine how Prometheus metrics are going to be exported
    let metrics_path = bot_cfg
        .telegram
        .webhook
        .iter()
        .find_map(|w| w.metrics_path.as_ref());
    let (metrics_server_cfg, webhook_metrics_config) = match (bot_cfg.metrics_server, metrics_path)
    {
        (Some(msc), None) => {
            // build configuration for the dedicated metrics exporter server
            let cfg = metrics::Config {
                reg: prom_registry,
                bind_addr: msc.bind_addr,
                path: msc.path,
            };
            (Some(cfg), None)
        }
        (None, Some(path)) => {
            // build configuration metrics exporter embedded in the webhook server
            let cfg = webhook::MetricsConfig {
                reg: prom_registry,
                path: path.clone(),
            };
            (None, Some(cfg))
        }
        (Some(_), Some(_)) => {
            panic!("configuration error: metrics_server and telegram.webhook.metrics_path cannot be both set");
        }
        _ => (None, None),
    };

    // optionally, start the dedicated metrics exporting server
    let metrics_handles = metrics_server_cfg.map(|cfg| {
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
        (
            cancel_tx,
            tokio::spawn(metrics::start_server(cfg, cancel_rx)),
        )
    });

    // start webhook server or update poller
    match (bot_cfg.telegram.webhook, bot_cfg.telegram.polling) {
        (Some(webhook), None) => {
            log::debug!("starting webhook server");
            webhook::start(webhook::Config {
                path: webhook.path,
                secret_token: webhook.secret_token,
                bind_addr: webhook.bind_addr,
                output: updates_tx,
                accept_timeout: webhook.accept_timeout,
                cancel: cancel_rx,
                metrics: webhook_metrics_config,
            })
            .await;
            log::debug!("webhook server finished");
        }
        (None, Some(polling)) => {
            use crate::telegram::polling;
            log::debug!("starting polling manager");
            let telegram_client =
                TelegramClient::new(bot_cfg.telegram.api_token, polling.http_timeout);
            polling::start(
                telegram_client,
                polling::Config {
                    output: updates_tx,
                    duration_seconds: polling.duration_seconds,
                    cancel: cancel_rx,
                },
            )
            .await;
            log::debug!("polling manager finished")
        }
        (_, _) => {
            panic!("configuration error: either webhook or polling must be specified");
        }
    }

    // when the source of updates finishes, wait for the cancellation to propagate to the other tasks
    watcher_join.await.unwrap();
    output_join.await.unwrap();
    bot_join.await.unwrap();
    if let Some((cancel_tx, join_handle)) = metrics_handles {
        cancel_tx.send(()).await.unwrap();
        join_handle.await.unwrap();
    }

    log::debug!("all tasks finished");
}
