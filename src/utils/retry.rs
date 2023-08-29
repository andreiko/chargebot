use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use backoff::backoff::Backoff;

/// Returns retry backoff profile commonly used in this app.
/// Attempts will be happening indefinitely, with exponentially increasing intervals up to 10 seconds.
pub fn exp_backoff_forever() -> impl Backoff {
    backoff::ExponentialBackoffBuilder::default()
        .with_max_elapsed_time(None)
        .with_max_interval(Duration::from_secs(10))
        .build()
}

/// Retries asynchronous function `f` using backoff profile `b`. Calls `test_fn` to determine
/// whether the error should be retried. If the error should not be retried, it's returned back
/// to the caller. Otherwise `log_fn` is called to let the client log the intermittent error.
pub async fn retry<B, T, E, F, FUT, TF, LF>(
    b: &mut B,
    f: F,
    test_fn: TF,
    log_fn: LF,
) -> Result<T, E>
where
    B: Backoff,
    E: Display,
    F: Fn() -> FUT,
    FUT: Future<Output = Result<T, E>>,
    TF: Fn(&E) -> bool,
    LF: Fn(&E),
{
    loop {
        match f().await {
            Ok(res) => break Ok(res),
            Err(err) => match (test_fn(&err), b.next_backoff()) {
                (true, Some(delay)) => {
                    log_fn(&err);
                    tokio::time::sleep(delay).await;
                }
                _ => return Err(err),
            },
        }
    }
}
