//! The commands a claimed place runs with `exec_in_box`, kept past the call that started them.
//!
//! A command runs on a thread of its own through the `exec` op, so it holds the Box as a run and
//! leaves its `exec_ran` event as any exec does. Its output goes to a capped buffer with a read
//! mark per stream. Each answer waits up to a window for the command to end, then gives the
//! output since the last answer, so no call outlasts an agent's tool timeout. A run is kept a
//! while after it ends, and each answer on it gives its end again. A controller restart forgets
//! every run.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use moor_core::request::BoxRunId;
use moor_core::seam::proc::Pipes;
use moor_core::user::UserId;
use moor_core::view::{BoxRunState, BoxRunView, ExecView};
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;

/// How long one answer waits for its command to end: under the minute an agent's client waits.
pub(crate) const WINDOW: Duration = Duration::from_secs(45);

/// How long a run stays after it ends, for an answer that asks again.
const KEEP_ENDED: Duration = Duration::from_mins(10);

/// Bytes of stdout, and of stderr, a run keeps.
const OUTPUT_CAP: usize = 64 * 1024;

/// How often a waiting answer looks at the clock and the drain.
const TICK: Duration = Duration::from_millis(100);

/// The refusal for a run that is not the caller's, has ended long ago, or never was.
pub(crate) const NO_RUN: &str = "no such run of your user's: it ended over 10 minutes ago, the \
     controller restarted, or it never was. Run the command again with exec_in_box.";

/// One stream of a run: the bytes kept, whether more were cut, and how far answers have read.
#[derive(Default)]
struct Stream {
    /// The first [`OUTPUT_CAP`] bytes written.
    bytes: Vec<u8>,
    /// Whether bytes past the cap were dropped.
    cut: bool,
    /// How many of `bytes` an answer has given.
    read: usize,
}

impl Stream {
    /// Keeps what of `buf` fits under the cap, noting any cut.
    fn push(&mut self, buf: &[u8]) {
        let room = OUTPUT_CAP.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(buf.get(..room.min(buf.len())).unwrap_or_default());
        self.cut |= buf.len() > room;
    }

    /// The bytes since the last read as text, moving the mark past them. While the command
    /// runs a character it has written only part of stays for the next read.
    fn take(&mut self, ended: bool) -> String {
        let new = self.bytes.get(self.read..).unwrap_or_default();
        let whole = match std::str::from_utf8(new) {
            Err(e) if !ended && e.error_len().is_none() => e.valid_up_to(),
            _ => new.len(),
        };
        let text = String::from_utf8_lossy(new.get(..whole).unwrap_or_default()).into_owned();
        self.read += whole;
        text
    }
}

/// What a run has written and how it ended.
#[derive(Default)]
struct State {
    /// The command's stdout.
    stdout: Stream,
    /// The command's stderr.
    stderr: Stream,
    /// How the command ended, or why it never ran; none while it runs.
    end: Option<Result<ExecView, (ErrorKind, String)>>,
    /// When it ended.
    ended_at: Option<Instant>,
}

/// One command of a place's user.
struct Run {
    /// The place's user, the only one whose answers read it.
    user: UserId,
    /// What it wrote and how it ended.
    state: Mutex<State>,
    /// Notified once it ends.
    ended: Condvar,
}

impl Run {
    /// Its state, locked.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The runs of one process, by state directory and id.
type Runs = Mutex<HashMap<(PathBuf, BoxRunId), Arc<Run>>>;

/// The runs in this process, by state directory and id.
fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(Mutex::default)
}

/// The key of run `id` in `cx`'s state directory.
fn key(cx: &Cx, id: BoxRunId) -> (PathBuf, BoxRunId) {
    (cx.state_dir().map(PathBuf::from).unwrap_or_default(), id)
}

/// A new run's handle, which carries its command's output and end.
pub(crate) struct Started {
    /// The run's id.
    pub(crate) id: BoxRunId,
    /// The run the command writes to.
    run: Arc<Run>,
}

/// Registers a run of `user` in `cx`'s state directory, forgetting runs that ended long ago.
/// Its id is random, so an id from before a restart never names a new run.
pub(crate) fn register(cx: &Cx, user: UserId) -> Started {
    let now = cx.clock().instant();
    let run = Arc::new(Run {
        user,
        state: Mutex::default(),
        ended: Condvar::new(),
    });
    let mut all = runs().lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|_, r| {
        r.state()
            .ended_at
            .is_none_or(|at| now.saturating_duration_since(at) < KEEP_ENDED)
    });
    let id = loop {
        let id = BoxRunId(cx.rng().u32());
        if id.0 != 0 && !all.contains_key(&key(cx, id)) {
            break id;
        }
    };
    all.insert(key(cx, id), Arc::clone(&run));
    drop(all);
    Started { id, run }
}

/// Writes one stream of a run.
struct Into {
    /// The run written to.
    run: Arc<Run>,
    /// Whether this is its stderr.
    stderr: bool,
}

impl Write for Into {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.run.state();
        if self.stderr {
            state.stderr.push(buf);
        } else {
            state.stdout.push(buf);
        }
        drop(state);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Started {
    /// Runs `exec` with its output into this run, stopping it once `stop` says so, and records
    /// how it ended at `clock`'s instant.
    pub(crate) fn run(
        self,
        now: impl Fn() -> Instant,
        stop: &(dyn Fn() -> bool + Sync),
        exec: impl FnOnce(Pipes<'_>) -> Result<ExecView, CommandError>,
    ) {
        let (mut stdout, mut stderr) = (
            Into {
                run: Arc::clone(&self.run),
                stderr: false,
            },
            Into {
                run: Arc::clone(&self.run),
                stderr: true,
            },
        );
        let ended = exec(Pipes {
            stdin: Box::new(io::empty()),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stop,
        });
        let mut state = self.run.state();
        state.end = Some(ended.map_err(|e| (e.kind, e.error.to_string())));
        state.ended_at = Some(now());
        drop(state);
        self.run.ended.notify_all();
    }
}

/// Waits up to `window` for run `id` of `user` to end, or until `stop` says so, then answers
/// with the output since the last answer and, once the run has ended, how.
///
/// # Errors
/// [`NO_RUN`] for a run of another user or none, and the error of a command that never ran.
pub(crate) fn answer(
    cx: &Cx,
    user: UserId,
    id: BoxRunId,
    window: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<BoxRunView, CommandError> {
    let run = runs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key(cx, id))
        .filter(|r| r.user == user)
        .cloned()
        .ok_or_else(|| CommandError::new(ErrorKind::Refused, NO_RUN))?;
    let clock = cx.clock();
    let until = clock.instant() + window;
    let mut state = run.state();
    while state.end.is_none() && clock.instant() < until && !stop() {
        state = run
            .ended
            .wait_timeout(state, TICK)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    let ended = state.end.is_some();
    let (stdout, stderr) = (state.stdout.take(ended), state.stderr.take(ended));
    let now = match &state.end {
        None => BoxRunState::Running,
        Some(Ok(view)) => BoxRunState::from(*view),
        Some(Err((kind, why))) => return Err(CommandError::new(*kind, why.clone())),
    };
    Ok(BoxRunView {
        run: id,
        state: now,
        stdout,
        stderr,
        stdout_truncated: state.stdout.cut,
        stderr_truncated: state.stderr.cut,
    })
}
