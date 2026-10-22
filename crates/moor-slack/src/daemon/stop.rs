//! Stopping the daemon: waits for the threads and sleeps in slices a stop can cut short.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Waits for the Socket Mode thread to end. When that was a stop, waits for the worker to
/// finish its queue and returns `Ok`; otherwise the worker stopped taking mentions.
pub(super) fn finish(
    stopping: &AtomicBool,
    intake: JoinHandle<()>,
    jobs: JoinHandle<()>,
) -> Result<(), String> {
    intake
        .join()
        .map_err(|_| "the Socket Mode thread panicked".to_owned())?;
    if !stopping.load(Ordering::SeqCst) {
        return Err("the worker stopped taking mentions".to_owned());
    }
    jobs.join()
        .map_err(|_| "the worker thread panicked".to_owned())
}

/// Sleeps for `total`, or until `stop` is set.
pub(super) fn sleep_until_stopped(stop: &AtomicBool, total: Duration) {
    const SLICE: Duration = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let nap = left.min(SLICE);
        thread::sleep(nap);
        left -= nap;
    }
}
