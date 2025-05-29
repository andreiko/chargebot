use std::{future::Future, time::Duration};

use backoff::{backoff::Backoff, ExponentialBackoff, ExponentialBackoffBuilder};
use tokio::time::sleep;

pub trait MaybeRetriable {
    fn is_retriable(&self) -> bool;
}

/// Returns retry backoff profile commonly used in this app.
/// Attempts will be happening indefinitely, with exponentially increasing intervals up to 10 seconds.
#[must_use]
pub fn exp_backoff_forever() -> ExponentialBackoff {
    const INITIAL_INTERVAL: Duration = Duration::from_millis(500);
    const MAX_INTERVAL: Duration = Duration::from_secs(10);
    ExponentialBackoffBuilder::default()
        .with_initial_interval(INITIAL_INTERVAL)
        .with_max_interval(MAX_INTERVAL)
        .with_max_elapsed_time(None)
        .build()
}

/// Executes `f()` and propagates its successful return value.
/// Retries errors that are considered retriable by `MaybeRetriable` trait.
/// Calls `log_fn()` after each retriable error.
///
/// # Errors
/// If `f()` returns a non-retriable error.
/// Backoff manager returned by `backoff_fn()` runs out of attempts/time.
pub async fn retry<BF, F, LF, T, E, FUT>(backoff_fn: BF, f: F, log_fn: LF) -> Result<T, E>
where
    BF: Fn() -> ExponentialBackoff + Copy,
    F: Fn() -> FUT,
    LF: Fn(E),
    FUT: Future<Output = Result<T, E>>,
    E: MaybeRetriable,
{
    let mut backoff = None::<ExponentialBackoff>;
    loop {
        match f().await {
            Ok(val) => break Ok(val),
            Err(err) => {
                let b = backoff.get_or_insert_with(backoff_fn);
                match (err.is_retriable(), b.next_backoff()) {
                    (true, Some(delay)) => {
                        log_fn(err);
                        sleep(delay).await;
                    }
                    _ => return Err(err),
                }
            }
        }
    }
}
