//! Scripted failures and recorded calls for the Driver fake.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{
    BoxStep, Driver, DriverError, ExecOutcome, ExecSpec, SleepOutcome, StepDone,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath};

use super::{FakeDriver, State};

/// One `peek_file` call: handle, path and host cwd.
pub(super) type PeekCall = (BoxHandle, BoxPath, PathBuf);

/// One `put_file` call: handle, path, content, mode and host cwd.
pub(crate) type PutCall = (BoxHandle, BoxPath, Vec<u8>, u32, PathBuf);

/// Where the fake runs a Box's commands: `<root>/<lease id>/<repo name>`.
/// The lease id (`box-N` becomes `cbx_N`) differs from the handle, so a path
/// rebuilt from the handle is caught.
pub(super) fn layout(
    root: Option<&str>,
    handle: &str,
    worktree: Option<&Path>,
) -> Result<BoxPath, DriverError> {
    let lease = format!("cbx_{}", handle.strip_prefix("box-").unwrap_or(handle));
    let repo = worktree
        .and_then(Path::file_name)
        .map_or_else(|| "repo".into(), |n| n.to_string_lossy());
    BoxPath::under(root.unwrap_or("/work"), &format!("{lease}/{repo}")).map_err(|e| {
        DriverError::Failed {
            summary: e.to_string(),
        }
    })
}

/// Credential sweeps: the scripted outcome and the record.
#[derive(Default)]
pub(super) struct Sweeps {
    /// Every sweep: handle, spec and deadline.
    calls: Vec<(BoxHandle, ExecSpec, Deadline)>,
    /// Every sweep fails with this.
    fail: Option<DriverError>,
    /// How long a sweep takes on the Box.
    takes: Duration,
    /// Sweeps that ran to their end.
    finished: usize,
}

impl FakeDriver {
    /// Makes every credential sweep fail with `e`.
    pub(crate) fn fail_sweep(self, e: DriverError) -> Self {
        self.st().sweep.fail = Some(e);
        self
    }

    /// Makes every credential sweep take `took` on the Box; one with a shorter deadline times out.
    pub(crate) fn sweep_takes(self, took: Duration) -> Self {
        self.st().sweep.takes = took;
        self
    }

    /// Every credential sweep so far; kept out of [`FakeDriver::execs`].
    pub(crate) fn sweeps(&self) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
        self.st().sweep.calls.clone()
    }

    /// How many credential sweeps ran to their end.
    pub(crate) fn finished_sweeps(&self) -> usize {
        self.st().sweep.finished
    }

    pub(super) fn sweep_exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        let mut s = self.st();
        s.note(format!("sweep {}", handle.as_str()));
        s.sweep.calls.push((handle.clone(), spec.clone(), deadline));
        if let Some(e) = s.sweep.fail.clone() {
            return Err(e);
        }
        if deadline.0 < s.sweep.takes {
            return Err(DriverError::Timeout(deadline.0));
        }
        s.sweep.finished += 1;
        Ok(ExecOutcome::Exited { code: 0 })
    }

    /// Makes `peek_file` of `path` in the Box `handle` return `text`.
    pub(crate) fn peek_text(self, handle: &str, path: &BoxPath, text: &str) -> Self {
        self.set_peek_text(handle, path, text);
        self
    }

    /// Replaces what `peek_file` of `path` in the Box `handle` returns, even mid-run.
    pub(crate) fn set_peek_text(&self, handle: &str, path: &BoxPath, text: &str) {
        let mut s = self.st();
        s.files
            .retain(|(h, p, _)| h != handle || p != path.as_str());
        s.files
            .push((handle.to_owned(), path.as_str().to_owned(), text.to_owned()));
    }

    /// Queues one more scripted `exec` result on a driver already in use.
    pub(crate) fn push_exec_result(&self, r: Result<ExecOutcome, DriverError>) {
        self.st().exec_results.push_back(r);
    }

    /// The next `peek_file` fails once with `e`.
    pub(crate) fn fail_peek(self, e: DriverError) -> Self {
        self.st().fail_peek = Some(e);
        self
    }

    /// The next `put_file` fails once with `e`.
    pub(crate) fn fail_put(self, e: DriverError) -> Self {
        self.st().fail_put = Some(e);
        self
    }

    /// Makes the next `put_file` fail once with `e`, even mid-run.
    pub(crate) fn set_fail_put(&self, e: DriverError) {
        self.st().fail_put = Some(e);
    }

    /// The next `connect_argv` fails once with `e`.
    pub(crate) fn fail_connect(self, e: DriverError) -> Self {
        self.st().fail_connect = Some(e);
        self
    }

    /// Every `peek_file` so far: handle, path and host cwd, in call order.
    pub(crate) fn peeks(&self) -> Vec<PeekCall> {
        self.st().peeks.clone()
    }

    /// Every `put_file` so far, in call order.
    pub(crate) fn puts(&self) -> Vec<PutCall> {
        self.st().puts.clone()
    }

    /// The next `sleep` fails once with `e`.
    pub(crate) fn fail_sleep(self, e: DriverError) -> Self {
        self.st().fail_sleep = Some(e);
        self
    }

    /// The next `wake` fails once with `e`.
    pub(crate) fn fail_wake(self, e: DriverError) -> Self {
        self.st().fail_wake = Some(e);
        self
    }

    /// Runs `f` at the start of every later `exec`, outside the Driver's lock.
    pub(crate) fn on_exec(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().on_exec = Some(Arc::new(f));
    }

    /// Runs `f` at the end of every later `exec`, outside the Driver's lock.
    pub(crate) fn after_exec(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().after_exec = Some(Arc::new(f));
    }

    /// Runs `f` inside every later `wake` that succeeds, outside the Driver's lock; models a resume that cold-boots.
    pub(crate) fn on_wake(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().on_wake = Some(Arc::new(f));
    }

    /// Makes every `sleep` report `outcome` instead of `Slept`.
    pub(crate) fn sleep_outcome(self, outcome: SleepOutcome) -> Self {
        self.st().sleep_outcome = Some(outcome);
        self
    }
}

/// Recorded top-level Box calls, batch connects and the scripted `Run` stdout.
#[derive(Default)]
pub(super) struct Rec {
    in_batch: bool,
    box_calls: Vec<String>,
    connects: Vec<Vec<String>>,
    deadlines: Vec<Deadline>,
    run_stdout: String,
    run_queue: std::collections::VecDeque<String>,
    box_files: Vec<(String, Option<Vec<u8>>)>,
    fail_stream: Option<(usize, DriverError)>,
    streams: Vec<(BoxHandle, BoxPath, Vec<String>)>,
}

impl State {
    /// Logs `call` in `calls`; counts it as a top-level call unless a batch dispatches it.
    pub(super) fn note(&mut self, call: String) {
        if !self.rec.in_batch {
            let name = call.split_whitespace().next().unwrap_or_default();
            self.rec.box_calls.push(name.to_owned());
        }
        self.calls.push(call);
    }
}

/// Writes `bytes` to `to`; like a real copy, fails when the destination directory is missing.
pub(super) fn write_bytes(to: &Path, bytes: &[u8]) -> Result<(), DriverError> {
    std::fs::write(to, bytes).map_err(|e| DriverError::Failed {
        summary: format!("copy destination parent is unavailable: {e}"),
    })
}

impl FakeDriver {
    /// Makes every batched `Run` report `text` as its stdout.
    pub(crate) fn run_stdout(self, text: &str) -> Self {
        self.set_run_stdout(text);
        self
    }

    /// Makes the next batched `Run`s report these stdouts in order, then the usual one.
    pub(crate) fn run_stdouts<'a>(self, texts: impl IntoIterator<Item = &'a str>) -> Self {
        self.st()
            .rec
            .run_queue
            .extend(texts.into_iter().map(str::to_owned));
        self
    }

    /// Replaces the stdout every batched `Run` reports, even mid-run.
    pub(crate) fn set_run_stdout(&self, text: &str) {
        text.clone_into(&mut self.st().rec.run_stdout);
    }

    /// Driver methods invoked from outside so far, by name; a batch counts once.
    pub(crate) fn box_calls(&self) -> Vec<String> {
        self.st().rec.box_calls.clone()
    }

    /// The `connect` argv of every `batch` so far, in call order.
    pub(crate) fn batch_connects(&self) -> Vec<Vec<String>> {
        self.st().rec.connects.clone()
    }

    /// The deadline of every `batch` so far, in call order.
    pub(crate) fn batch_deadlines(&self) -> Vec<Deadline> {
        self.st().rec.deadlines.clone()
    }

    /// Runs `steps` in order through the fake's own `peek_file`, `exec` and `put_file`.
    pub(super) fn run_batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        steps: &[BoxStep],
        host_cwd: &Path,
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        {
            let mut s = self.st();
            s.rec.box_calls.push("batch".to_owned());
            s.rec.connects.push(connect.to_vec());
            s.rec.deadlines.push(deadline);
            s.rec.in_batch = true;
        }
        let mut out = Vec::new();
        for step in steps {
            let done = self.run_step(handle, step, host_cwd, deadline);
            let stop = match &done {
                Err(_) => true,
                Ok(StepDone::Ran { outcome, .. }) => *outcome != ExecOutcome::Exited { code: 0 },
                Ok(_) => false,
            };
            out.push(done);
            if stop {
                break;
            }
        }
        self.st().rec.in_batch = false;
        out
    }

    fn run_step(
        &self,
        handle: &BoxHandle,
        step: &BoxStep,
        host_cwd: &Path,
        deadline: Deadline,
    ) -> Result<StepDone, DriverError> {
        match step {
            BoxStep::Peek(path) => self.peek_file(handle, path, host_cwd).map(StepDone::Peeked),
            BoxStep::Put { path, bytes, mode } => self
                .put_file(handle, path, bytes, *mode, host_cwd)
                .map(|()| StepDone::Put),
            BoxStep::Run(script) => {
                let spec = ExecSpec {
                    argv: vec!["sh".to_owned(), "-c".to_owned(), script.clone()],
                    env: vec![],
                    cwd: None,
                    sync: SyncMode::None,
                    output: None,
                    host_cwd: Some(host_cwd.to_path_buf()),
                    class: None,
                };
                let outcome = self.exec(handle, &spec, deadline)?;
                let mut s = self.st();
                let stdout = if script.contains(".moor/artifacts") {
                    s.artifact_index.pop_front().unwrap_or_default()
                } else {
                    s.rec
                        .run_queue
                        .pop_front()
                        .unwrap_or_else(|| s.rec.run_stdout.clone())
                };
                Ok(StepDone::Ran { outcome, stdout })
            }
        }
    }
}

impl FakeDriver {
    /// A regular file `name` in the Box that `stream_files` can stream.
    pub(crate) fn box_file(self, name: &str, bytes: &[u8]) -> Self {
        self.put_box_file(name, bytes);
        self
    }

    /// Adds or replaces the streamable file `name`, even mid-run.
    pub(crate) fn put_box_file(&self, name: &str, bytes: &[u8]) {
        self.set_box_entry(name, Some(bytes.to_vec()));
    }

    /// An entry `name` that `stream_files` hands over as a non-regular file.
    pub(crate) fn box_special(self, name: &str) -> Self {
        self.set_box_entry(name, None);
        self
    }

    fn set_box_entry(&self, name: &str, bytes: Option<Vec<u8>>) {
        let mut s = self.st();
        s.rec.box_files.retain(|(n, _)| n != name);
        s.rec.box_files.push((name.to_owned(), bytes));
    }

    /// The next `stream_files` hands over its first `n` available files, then fails once with `err`.
    pub(crate) fn fail_stream_after(&self, n: usize, err: DriverError) {
        self.st().rec.fail_stream = Some((n, err));
    }

    /// Every `stream_files` so far: handle, directory and requested names, in call order.
    pub(crate) fn streams(&self) -> Vec<(BoxHandle, BoxPath, Vec<String>)> {
        self.st().rec.streams.clone()
    }

    /// Streams the configured entries among `names`; `Unsupported` when the Box has none.
    pub(super) fn run_stream(
        &self,
        handle: &BoxHandle,
        dir: &BoxPath,
        names: &[String],
        sink: &mut dyn FnMut(&str, Option<&mut dyn std::io::Read>),
    ) -> Result<(), DriverError> {
        let (entries, fail) = {
            let mut s = self.st();
            s.note(format!("stream_files {}", handle.as_str()));
            s.rec
                .streams
                .push((handle.clone(), dir.clone(), names.to_vec()));
            if !s.live.iter().any(|h| h == handle.as_str()) {
                return Err(DriverError::NotFound);
            }
            if s.rec.box_files.is_empty() {
                return Err(DriverError::Unsupported("streaming files"));
            }
            let entries: Vec<_> = names
                .iter()
                .filter_map(|n| {
                    let (_, b) = s.rec.box_files.iter().find(|(f, _)| f == n)?;
                    Some((n.clone(), b.clone()))
                })
                .collect();
            (entries, s.rec.fail_stream.take())
        };
        let limit = fail.as_ref().map_or(usize::MAX, |(n, _)| *n);
        for (i, (name, bytes)) in entries.iter().enumerate() {
            if i >= limit {
                break;
            }
            match bytes {
                Some(b) => {
                    let mut r: &[u8] = b;
                    sink(name, Some(&mut r));
                }
                None => sink(name, None),
            }
        }
        fail.map_or(Ok(()), |(_, e)| Err(e))
    }
}
