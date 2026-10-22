//! Follows a run log while its command runs, handing each new piece to a sink.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use moor_core::controller::BYTES_MAX;
use moor_core::seam::proc::Output;

/// Receives the command's output piece by piece; an error stops the following, not the run.
pub(crate) type Sink<'a> = &'a mut (dyn FnMut(&[u8]) -> io::Result<()> + Send);

/// Pause between reads that found no new bytes.
const POLL: Duration = Duration::from_millis(50);

/// Runs `work` while passing each new piece of the log behind `output` to `sink`.
///
/// Returns once `work` has returned and the log is read to its end. Without a log or a sink
/// it only runs `work`.
pub(super) fn during<T>(
    output: Option<&Output>,
    sink: Option<Sink<'_>>,
    work: impl FnOnce() -> T,
) -> T {
    let (Some(Output::Log(path)), Some(sink)) = (output, sink) else {
        return work();
    };
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| follow(path, &done, sink));
        let _done = SetOnDrop(&done);
        work()
    })
}

/// Sets its flag when dropped, so a thread waiting on the flag ends even if the work panics.
pub(super) struct SetOnDrop<'a>(pub(super) &'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Copies `path` to `sink` as it grows, until `done` is set and no bytes remain.
///
/// Holds at most [`BYTES_MAX`] bytes; the file may not exist until the command starts.
fn follow(path: &Path, done: &AtomicBool, sink: Sink<'_>) {
    let mut file = None;
    let mut buf = vec![0; BYTES_MAX];
    loop {
        let finished = done.load(Ordering::SeqCst);
        if file.is_none() {
            file = File::open(path).ok();
        }
        let read = file.as_mut().map_or(Ok(0), |f| f.read(&mut buf));
        match read {
            Ok(0) if finished => return,
            Ok(0) => thread::sleep(POLL),
            Ok(n) => {
                if sink(buf.get(..n).unwrap_or_default()).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}
