//! One link the agent holds: serving the runner's streams on it until it ends or the runner asks
//! for a new one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::{self, Frame};
use tokio::sync::{mpsc, oneshot};

use crate::{listen, pool, serve, status, Machine, Pool};

/// Serves the runner's streams on `carried` until it ends, and why; none once the runner asked
/// for a new link, with this one left to close once its streams finish.
pub(crate) async fn hold(
    carried: link::Carried,
    machine: &Arc<Machine>,
    boxes: &Arc<listen::Boxes>,
    recent: &Arc<status::Recent>,
) -> Option<String> {
    let (open, asks) = mpsc::channel(16);
    let reporting = machine
        .pool
        .clone()
        .map(|pool| tokio::spawn(report_on(open.clone(), machine.home.clone(), pool)));
    let streams = listen::Link {
        open,
        boxes: Arc::clone(boxes),
    };
    let (machine, recent) = (Arc::clone(machine), Arc::clone(recent));
    let take = move |ch| {
        let (machine, streams) = (Arc::clone(&machine), streams.clone());
        let recent = Arc::clone(&recent);
        tokio::spawn(async move { serve::serve(&machine, &streams, &recent, ch).await });
    };
    let (drain, draining) = oneshot::channel();
    let drained = async {
        if draining.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let mut driving = tokio::spawn(link::drive(
        carried.socket,
        link::Mode::Client,
        asks,
        take,
        drained,
    ));
    let mut ends = Ends(Some(driving.abort_handle()));
    let ended = tokio::select! {
        why = &mut driving => Some(why.unwrap_or_else(|e| e.to_string())),
        () = relink(carried.heard) => None,
    };
    if let Some(r) = reporting {
        r.abort();
    }
    if ended.is_none() {
        let _ = drain.send(());
        ends.0 = None;
    }
    ended
}

/// Ends the link's yamux when dropped, as when the agent stops, unless it was let go to drain.
struct Ends(Option<tokio::task::AbortHandle>);

impl Drop for Ends {
    fn drop(&mut self) {
        if let Some(driving) = &self.0 {
            driving.abort();
        }
    }
}

/// Resolves when the runner sends `relink` among `heard`; never once the link is gone.
async fn relink(mut heard: mpsc::Receiver<Frame>) {
    while let Some(frame) = heard.recv().await {
        if frame == (Frame::Relink {}) {
            return;
        }
    }
    std::future::pending::<()>().await;
}

/// Opens a stream to the runner on `open` and sends the host's memory reports on it.
async fn report_on(open: mpsc::Sender<link::Asked>, home: PathBuf, pool: Pool) {
    let (ask, opened) = tokio::sync::oneshot::channel();
    if open.send(ask).await.is_err() {
        return;
    }
    if let Ok(Ok(stream)) = opened.await {
        pool::send_reports(stream, Path::new("/proc"), Some(&home), Some(&pool)).await;
    }
}
