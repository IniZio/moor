//! One agent process: its own process group, input on stdin written after the spawn, its output read line by line, and the whole group killed when the
//! watcher is done, so nothing outlives the run.

use std::io::Write as _;
use std::io::{self, BufRead as _, BufReader, Read};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{kill_process_group, Pid, Signal};

/// How long a process that closed its output may take to exit before its group is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Most bytes kept from one output stream; the rest is read and dropped.
const MAX_OUTPUT: usize = 1 << 20;

/// The running process's standard output, one line at a time.
pub(super) struct Lines {
    /// Lines as the reader thread reads them, each without its newline.
    rx: Receiver<String>,
    /// The process id, which is also its process group id.
    pub(super) pid: u32,
}

/// What [`Lines::next`] found.
pub(super) enum Next {
    /// One line of output.
    Line(String),
    /// The process closed its output.
    End,
    /// `deadline` passed first.
    Late,
}

impl Lines {
    /// The next line, waiting no later than `deadline`.
    pub(super) fn next(&self, deadline: Instant) -> Next {
        match self
            .rx
            .recv_timeout(deadline.saturating_duration_since(now()))
        {
            Ok(line) => Next::Line(line),
            Err(RecvTimeoutError::Timeout) => Next::Late,
            Err(RecvTimeoutError::Disconnected) => Next::End,
        }
    }
}

/// How the watcher left the process.
pub(super) enum Left {
    /// It saw the output end; the process gets a short grace to exit.
    Ended,
    /// It stopped watching early; the group is killed at once.
    Stopped,
}

/// Runs `cmd` in its own process group with `input` on stdin and hands its output to `watch`. When `watch` returns, any process the group still
/// holds is killed. Returns what `watch` returned and the first [`MAX_OUTPUT`] bytes of stderr.
pub(super) fn supervise<T>(
    mut cmd: Command,
    input: &[u8],
    watch: impl FnOnce(&Lines) -> (T, Left),
) -> io::Result<(T, Vec<u8>)> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let group = Pid::from_child(&child);
    thread::scope(|scope| {
        let stdin = child.stdin.take();
        scope.spawn(move || stdin.map(|mut stdin| stdin.write_all(input)));
        let (tx, rx) = mpsc::channel();
        let stdout = child.stdout.take();
        scope.spawn(move || read_lines(stdout, &tx));
        let stderr = child.stderr.take();
        let err = scope.spawn(move || read_capped(stderr));
        let lines = Lines {
            rx,
            pid: child.id(),
        };
        let (out, left) = watch(&lines);
        if matches!(left, Left::Ended) {
            wait_up_to(&mut child, EXIT_GRACE);
        }
        let _ = kill_process_group(group, Signal::KILL);
        child.wait()?;
        Ok((out, err.join().unwrap_or_default()))
    })
}

/// The current instant; deadlines are waited on in real time.
#[allow(
    clippy::disallowed_methods,
    reason = "process limits are waited on with recv_timeout, which runs on real time"
)]
pub(super) fn now() -> Instant {
    Instant::now()
}

/// Waits for `child` to exit, at most `limit`; an error from the wait ends it early.
fn wait_up_to(child: &mut Child, limit: Duration) {
    let step = Duration::from_millis(20);
    let mut waited = Duration::ZERO;
    while matches!(child.try_wait(), Ok(None)) && waited < limit {
        thread::sleep(step);
        waited += step;
    }
}

/// Sends each line of `stream`, cut to its first [`MAX_OUTPUT`] bytes, to `tx` until
/// [`MAX_OUTPUT`] bytes were sent, then reads the rest and drops it, so the child never blocks
/// on a full pipe and a line with no end never grows past the cap.
fn read_lines(stream: Option<impl Read>, tx: &mpsc::Sender<String>) {
    let Some(stream) = stream else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut sent = 0_usize;
    let mut line = Vec::new();
    let mut deliver = |line: &mut Vec<u8>| {
        if sent < MAX_OUTPUT {
            sent += line.len();
            let text = String::from_utf8_lossy(line);
            let _ = tx.send(text.trim_end_matches(['\n', '\r']).to_owned());
        }
        line.clear();
    };
    while let Some(buf) = reader.fill_buf().ok().filter(|b| !b.is_empty()) {
        let end = buf.iter().position(|b| *b == b'\n');
        let (chunk, _) = buf.split_at(end.map_or(buf.len(), |at| at + 1));
        let room = MAX_OUTPUT.saturating_sub(line.len());
        line.extend(chunk.iter().take(room));
        let used = chunk.len();
        reader.consume(used);
        if end.is_some() {
            deliver(&mut line);
        }
    }
    if !line.is_empty() {
        deliver(&mut line);
    }
}

/// Reads `stream` to its end and keeps the first [`MAX_OUTPUT`] bytes.
fn read_capped(stream: Option<impl Read>) -> Vec<u8> {
    let Some(mut stream) = stream else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    while let Ok(n) = stream.read(&mut chunk) {
        let Some(read) = chunk.get(..n).filter(|r| !r.is_empty()) else {
            break;
        };
        let room = MAX_OUTPUT.saturating_sub(kept.len());
        kept.extend(read.iter().take(room));
    }
    kept
}

#[cfg(test)]
mod tests;
