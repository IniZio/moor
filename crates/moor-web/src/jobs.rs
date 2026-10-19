//! Background jobs: long controller requests run off the request thread and stream progress.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;
use std::io::{self, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::http::{Handler, HttpResponse, Reply};
use crate::sse::KEEPALIVE;

#[cfg(test)]
mod tests;

/// Finished jobs kept for late readers; older ones are dropped first.
const KEEP_FINISHED: usize = 200;

/// Progress lines kept per job; the oldest are dropped first.
const KEEP_PROGRESS: usize = 500;

/// Idle time between keepalive comments on a job stream.
const KEEPALIVE_EVERY: Duration = Duration::from_secs(15);

/// Process-wide job counter, mixed into every id so ids never repeat.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Why a job failed: the step that broke and what went wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobFailure {
    /// The step that failed.
    pub step: String,
    /// What went wrong.
    pub message: String,
}

/// How a job stands.
#[derive(Debug, Clone)]
enum Outcome {
    /// Still working.
    Running,
    /// Finished with a result.
    Done(Value),
    /// Finished with a failure.
    Failed(JobFailure),
}

/// One job's record.
#[derive(Debug)]
struct Job {
    /// Short human label.
    label: String,
    /// Newest progress lines, at most `KEEP_PROGRESS`.
    progress: VecDeque<String>,
    /// Lines ever reported, including dropped ones.
    seen: usize,
    /// Current outcome.
    outcome: Outcome,
}

impl Job {
    /// Appends a line, dropping the oldest when over the cap.
    fn push(&mut self, line: String) {
        self.progress.push_back(line);
        self.seen = self.seen.saturating_add(1);
        while self.progress.len() > KEEP_PROGRESS {
            self.progress.pop_front();
        }
    }

    /// Absolute number of the oldest kept line.
    fn first(&self) -> usize {
        self.seen.saturating_sub(self.progress.len())
    }
}

/// All jobs, guarded by one lock.
#[derive(Debug, Default)]
struct State {
    /// Jobs by id.
    jobs: HashMap<String, Job>,
    /// Ids of finished jobs, oldest first.
    finished: VecDeque<String>,
}

/// Shared state plus the signal that wakes streams.
type Shared = Arc<(Mutex<State>, Condvar)>;

/// Locks the state, ignoring poisoning.
fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registry of background jobs.
#[derive(Clone, Debug)]
pub struct Jobs {
    /// Jobs and the wake signal.
    shared: Shared,
    /// Idle time between keepalives on streams.
    keepalive: Duration,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

/// Handed to a job's work so it can report progress.
#[derive(Debug)]
pub struct JobHandle {
    /// The registry the job lives in.
    shared: Shared,
    /// This job's id.
    id: String,
}

impl JobHandle {
    /// Appends a progress line and wakes streams following the job.
    pub fn progress(&self, message: &str) {
        if let Some(job) = lock(&self.shared).jobs.get_mut(&self.id) {
            job.push(message.to_owned());
        }
        self.shared.1.notify_all();
    }
}

/// A fresh id: a keyed hash of a counter, so it is unique and not guessable.
fn new_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = RandomState::new().hash_one(n);
    format!("{mixed:016x}{n:x}")
}

/// Text of a panic payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "job panicked".to_owned())
}

impl Jobs {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::default(),
            keepalive: KEEPALIVE_EVERY,
        }
    }

    /// Sets the idle time between keepalives on streams.
    #[cfg(test)]
    const fn with_keepalive(mut self, every: Duration) -> Self {
        self.keepalive = every;
        self
    }

    /// Runs `work` on its own thread; returns the job id at once.
    pub fn spawn<F>(&self, label: &str, work: F) -> String
    where
        F: FnOnce(&JobHandle) -> Result<Value, JobFailure> + Send + 'static,
    {
        let id = new_id();
        lock(&self.shared).jobs.insert(
            id.clone(),
            Job {
                label: label.to_owned(),
                progress: VecDeque::new(),
                seen: 0,
                outcome: Outcome::Running,
            },
        );
        let handle = JobHandle {
            shared: Arc::clone(&self.shared),
            id: id.clone(),
        };
        let shared = Arc::clone(&self.shared);
        let run = move || {
            let outcome = match catch_unwind(AssertUnwindSafe(|| work(&handle))) {
                Ok(Ok(value)) => Outcome::Done(value),
                Ok(Err(failure)) => Outcome::Failed(failure),
                Err(payload) => Outcome::Failed(JobFailure {
                    step: "internal".to_owned(),
                    message: panic_text(payload.as_ref()),
                }),
            };
            finish(&handle.shared, &handle.id, outcome);
        };
        if let Err(err) = thread::Builder::new()
            .name("moor-web-job".into())
            .spawn(run)
        {
            let failure = JobFailure {
                step: "internal".to_owned(),
                message: format!("could not start job thread: {err}"),
            };
            finish(&shared, &id, Outcome::Failed(failure));
        }
        id
    }

    /// The job as JSON, or `None` for an unknown id.
    #[must_use]
    pub fn snapshot(&self, id: &str) -> Option<Value> {
        let state = lock(&self.shared);
        let job = state.jobs.get(id)?;
        let mut out = json!({
            "id": id,
            "label": job.label,
            "state": match job.outcome {
                Outcome::Running => "running",
                Outcome::Done(_) => "done",
                Outcome::Failed(_) => "failed",
            },
            "progress": job.progress,
        });
        let dropped = job.first();
        let extra = match &job.outcome {
            Outcome::Running => None,
            Outcome::Done(result) => Some(("result", result.clone())),
            Outcome::Failed(failure) => Some(("failure", json!(failure))),
        };
        drop(state);
        if let (true, Some(map)) = (dropped > 0, out.as_object_mut()) {
            map.insert("dropped".to_owned(), json!(dropped));
        }
        if let (Some((key, value)), Some(map)) = (extra, out.as_object_mut()) {
            map.insert(key.to_owned(), value);
        }
        Some(out)
    }

    /// Serves `GET /jobs/{id}` and `GET /jobs/{id}/events`; everything else goes to `fallback`.
    #[must_use]
    pub fn handler(&self, fallback: Handler) -> Handler {
        let jobs = self.clone();
        Arc::new(move |req| {
            jobs.route(&req.method, &req.path)
                .unwrap_or_else(|| fallback(req))
        })
    }

    /// Answers a job route, or `None` when the request is not one.
    fn route(&self, method: &str, path: &str) -> Option<Reply> {
        if method != "GET" {
            return None;
        }
        let rest = path.strip_prefix("/jobs/")?;
        let (id, events) = rest
            .strip_suffix("/events")
            .map_or((rest, false), |id| (id, true));
        if id.is_empty() || id.contains('/') {
            return None;
        }
        let Some(snapshot) = self.snapshot(id) else {
            return Some(HttpResponse::text("404 Not Found", "no such job").into());
        };
        if events {
            let jobs = self.clone();
            let id = id.to_owned();
            return Some(Reply::Stream(Box::new(move |out| jobs.stream(&id, out))));
        }
        let mut ok = HttpResponse::text("200 OK", snapshot.to_string());
        ok.content_type = "application/json";
        Some(ok.into())
    }

    /// Writes the job's progress and final frame to `out`, then returns.
    fn stream(&self, id: &str, out: &mut dyn Write) -> io::Result<()> {
        let mut sent = 0;
        loop {
            let (lines, dropped, outcome) = {
                let mut state = lock(&self.shared);
                loop {
                    let Some(job) = state.jobs.get(id) else {
                        return Ok(());
                    };
                    let first = job.first();
                    let start = sent.max(first);
                    let fresh: Vec<String> = job
                        .progress
                        .iter()
                        .skip(start.saturating_sub(first))
                        .cloned()
                        .collect();
                    if !fresh.is_empty() || !matches!(job.outcome, Outcome::Running) {
                        break (fresh, start.saturating_sub(sent), job.outcome.clone());
                    }
                    let (guard, timeout) = self
                        .shared
                        .1
                        .wait_timeout(state, self.keepalive)
                        .unwrap_or_else(PoisonError::into_inner);
                    state = guard;
                    if timeout.timed_out() {
                        drop(state);
                        out.write_all(KEEPALIVE)?;
                        out.flush()?;
                        state = lock(&self.shared);
                    }
                }
            };
            sent = sent.saturating_add(dropped).saturating_add(lines.len());
            if dropped > 0 {
                let note = format!("{dropped} earlier lines dropped");
                let data = serde_json::to_string(&note).unwrap_or_default();
                write!(out, "event: progress\ndata: {data}\n\n")?;
            }
            for line in &lines {
                let data = serde_json::to_string(line).unwrap_or_default();
                write!(out, "event: progress\ndata: {data}\n\n")?;
            }
            let running = matches!(outcome, Outcome::Running);
            let last = match outcome {
                Outcome::Running => None,
                Outcome::Done(result) => Some(format!("event: done\ndata: {result}\n\n")),
                Outcome::Failed(failure) => {
                    let data = serde_json::to_string(&failure).unwrap_or_default();
                    Some(format!("event: failed\ndata: {data}\n\n"))
                }
            };
            if let Some(frame) = last {
                out.write_all(frame.as_bytes())?;
            }
            out.flush()?;
            if !running {
                return Ok(());
            }
        }
    }
}

/// Records a job's outcome, trims old finished jobs and wakes streams.
fn finish(shared: &Shared, id: &str, outcome: Outcome) {
    let mut state = lock(shared);
    if let Some(job) = state.jobs.get_mut(id) {
        job.outcome = outcome;
        state.finished.push_back(id.to_owned());
    }
    while state.finished.len() > KEEP_FINISHED {
        if let Some(old) = state.finished.pop_front() {
            state.jobs.remove(&old);
        }
    }
    drop(state);
    shared.1.notify_all();
}
