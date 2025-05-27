use std::time::Duration;

use tokio::{select, sync::mpsc, time::sleep};

/// ensures the channel receives an item within 100ms and returns the item
pub async fn expect_recv<T>(rx: &mut mpsc::Receiver<T>) -> Option<T> {
    select! {
        item = rx.recv() => item,
        _ = sleep(Duration::from_millis(100)) => panic!("didn't receive from channel within timeout"),
    }
}

/// ensures the channel does NOT receive an item within 100ms
pub async fn expect_no_recv<T>(rx: &mut mpsc::Receiver<T>) {
    select! {
        _ = rx.recv() => panic!("received an unexpected item from the channel"),
        _ = sleep(Duration::from_millis(100)) => {},
    }
}
