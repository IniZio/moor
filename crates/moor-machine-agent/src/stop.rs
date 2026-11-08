//! Stopping the agent when the host asks.

use crate::pool;

/// How work run by [`until_stopped`] ended.
pub enum Ended<T> {
    /// The work finished with this.
    Done(T),
    /// The host asked the agent to stop with this signal.
    Stopped(&'static str),
}

/// Runs `work` until it ends or the host asks the agent to stop with SIGTERM or SIGINT. A stop
/// ends every boot going on before it returns, so no crabbox outlives the agent.
///
/// # Errors
/// `work` failed, or the agent cannot watch for those signals.
pub async fn until_stopped<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<Ended<T>, String> {
    use tokio::signal::unix::{signal, SignalKind};
    let watch = |kind| signal(kind).map_err(|e| format!("cannot watch for signals: {e}"));
    let (mut term, mut int) = (
        watch(SignalKind::terminate())?,
        watch(SignalKind::interrupt())?,
    );
    let asked = tokio::select! {
        ended = work => return ended.map(Ended::Done),
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    };
    pool::stop_boots().await;
    Ok(Ended::Stopped(asked))
}
