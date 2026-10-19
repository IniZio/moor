//! Tests for background jobs.

#![allow(clippy::unwrap_used, reason = "tests fail loudly on bad fixtures")]
#![allow(clippy::indexing_slicing, reason = "tests index JSON fixtures")]
#![allow(clippy::panic, reason = "a test job panics on purpose")]

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

/// Upper bound for anything a test waits on.
const PATIENCE: Duration = Duration::from_secs(10);

/// A job that blocks until released, so tests see it running.
fn gated(
    jobs: &Jobs,
) -> (
    String,
    mpsc::Sender<Result<Value, JobFailure>>,
    mpsc::Receiver<()>,
) {
    let (release, gate) = mpsc::channel::<Result<Value, JobFailure>>();
    let (step_tx, step_rx) = mpsc::channel();
    let id = jobs.spawn("gated", move |h| {
        h.progress("first");
        step_tx.send(()).unwrap();
        gate.recv().unwrap()
    });
    (id, release, step_rx)
}

/// Polls until `done` holds.
fn wait_for(mut done: impl FnMut() -> bool) {
    let mut left = 2000;
    while !done() {
        left -= 1;
        assert!(left > 0, "timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Waits for a job to leave `running`.
fn settle(jobs: &Jobs, id: &str) -> Value {
    wait_for(|| jobs.snapshot(id).unwrap()["state"] != "running");
    jobs.snapshot(id).unwrap()
}

/// A writer feeding a channel, so tests can read the stream while it runs.
struct ChannelWriter(mpsc::Sender<Vec<u8>>);

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .send(buf.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that always fails.
struct Gone;

impl Write for Gone {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads from `rx` until the accumulated text contains `needle`.
fn read_until(rx: &mpsc::Receiver<Vec<u8>>, acc: &mut String, needle: &str) {
    while !acc.contains(needle) {
        let chunk = rx.recv_timeout(PATIENCE).unwrap();
        acc.push_str(&String::from_utf8(chunk).unwrap());
    }
}

/// Opens the event stream of `id` on a thread, returning its output channel.
fn follow(jobs: &Jobs, id: &str) -> (mpsc::Receiver<Vec<u8>>, thread::JoinHandle<io::Result<()>>) {
    let (tx, rx) = mpsc::channel();
    let jobs = jobs.clone();
    let id = id.to_owned();
    let join = thread::spawn(move || jobs.stream(&id, &mut ChannelWriter(tx)));
    (rx, join)
}

/// Streams a settled job into a string.
fn stream_text(jobs: &Jobs, id: &str) -> String {
    let mut out = Vec::new();
    jobs.stream(id, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn running_job_shows_progress_then_done_with_result() {
    let jobs = Jobs::new();
    let (id, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    let snap = jobs.snapshot(&id).unwrap();
    assert_eq!(snap["state"], "running");
    assert_eq!(snap["label"], "gated");
    assert_eq!(snap["id"], id.as_str());
    assert_eq!(snap["progress"], json!(["first"]));
    assert!(snap.get("result").is_none());
    release.send(Ok(json!({"box": "b1"}))).unwrap();
    let snap = settle(&jobs, &id);
    assert_eq!(snap["state"], "done");
    assert_eq!(snap["result"], json!({"box": "b1"}));
    assert!(snap.get("failure").is_none());
}

#[test]
fn failure_carries_step_and_message() {
    let jobs = Jobs::new();
    let id = jobs.spawn("start", |h| {
        h.progress("cloning");
        Err(JobFailure {
            step: "setup".into(),
            message: "npm ci exited 1".into(),
        })
    });
    let snap = settle(&jobs, &id);
    assert_eq!(snap["state"], "failed");
    assert_eq!(
        snap["failure"],
        json!({"step": "setup", "message": "npm ci exited 1"})
    );
    assert_eq!(snap["progress"], json!(["cloning"]));
}

#[test]
fn panic_marks_job_failed_at_internal() {
    let jobs = Jobs::new();
    let id = jobs.spawn("boom", |_| panic!("kaboom"));
    let snap = settle(&jobs, &id);
    assert_eq!(snap["state"], "failed");
    assert_eq!(snap["failure"]["step"], "internal");
    assert_eq!(snap["failure"]["message"], "kaboom");
    let other = jobs.spawn("fine", |_| Ok(json!(1)));
    assert_eq!(settle(&jobs, &other)["state"], "done");
}

#[test]
fn ids_are_unique_over_a_thousand_spawns() {
    let jobs = Jobs::new();
    let ids: HashSet<String> = (0..1000)
        .map(|_| jobs.spawn("n", |_| Ok(json!(null))))
        .collect();
    assert_eq!(ids.len(), 1000);
    assert!(ids
        .iter()
        .all(|id| id.chars().all(|c| c.is_ascii_hexdigit())));
}

#[test]
fn unknown_job_has_no_snapshot() {
    assert!(Jobs::new().snapshot("nope").is_none());
}

#[test]
fn stream_replays_follows_and_ends_with_done() {
    let jobs = Jobs::new();
    let (id, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    let (rx, join) = follow(&jobs, &id);
    let mut seen = String::new();
    read_until(&rx, &mut seen, "event: progress\ndata: \"first\"\n\n");
    let handle = JobHandle {
        shared: Arc::clone(&jobs.shared),
        id,
    };
    handle.progress("second \"quoted\"");
    read_until(&rx, &mut seen, "data: \"second \\\"quoted\\\"\"\n\n");
    release.send(Ok(json!({"ok": true}))).unwrap();
    join.join().unwrap().unwrap();
    while let Ok(chunk) = rx.try_recv() {
        seen.push_str(&String::from_utf8(chunk).unwrap());
    }
    assert!(
        seen.ends_with("event: done\ndata: {\"ok\":true}\n\n"),
        "{seen}"
    );
}

#[test]
fn stream_of_finished_failed_job_replays_then_fails() {
    let jobs = Jobs::new();
    let id = jobs.spawn("start", |h| {
        h.progress("a");
        h.progress("b");
        Err(JobFailure {
            step: "boot".into(),
            message: "no".into(),
        })
    });
    settle(&jobs, &id);
    assert_eq!(
        stream_text(&jobs, &id),
        "event: progress\ndata: \"a\"\n\n\
         event: progress\ndata: \"b\"\n\n\
         event: failed\ndata: {\"step\":\"boot\",\"message\":\"no\"}\n\n"
    );
}

#[test]
fn stream_writes_keepalive_while_idle() {
    let jobs = Jobs::new().with_keepalive(Duration::from_millis(20));
    let (id, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    let (rx, join) = follow(&jobs, &id);
    let mut seen = String::new();
    read_until(&rx, &mut seen, ": keepalive\n\n");
    release.send(Ok(json!(0))).unwrap();
    join.join().unwrap().unwrap();
}

#[test]
fn stream_returns_when_the_reader_is_gone() {
    let jobs = Jobs::new().with_keepalive(Duration::from_millis(20));
    let (id, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    assert!(jobs.stream(&id, &mut Gone).is_err());
    release.send(Ok(json!(0))).unwrap();
}

#[test]
fn only_the_newest_finished_jobs_are_kept() {
    let jobs = Jobs::new();
    let first = jobs.spawn("first", |_| Ok(json!(0)));
    settle(&jobs, &first);
    let (running, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    let mut last = String::new();
    for _ in 0..KEEP_FINISHED {
        last = jobs.spawn("more", |_| Ok(json!(0)));
        settle(&jobs, &last);
    }
    wait_for(|| jobs.snapshot(&first).is_none());
    assert!(jobs.snapshot(&last).is_some());
    assert!(
        jobs.snapshot(&running).is_some(),
        "running jobs are never dropped"
    );
    assert_eq!(lock(&jobs.shared).finished.len(), KEEP_FINISHED);
    release.send(Ok(json!(0))).unwrap();
}

#[test]
fn routes_serve_snapshot_events_and_404() {
    let jobs = Jobs::new();
    let id = jobs.spawn("r", |h| {
        h.progress("x");
        Ok(json!(5))
    });
    settle(&jobs, &id);

    let Some(Reply::Full(resp)) = jobs.route("GET", &format!("/jobs/{id}")) else {
        panic!("expected a full reply");
    };
    assert_eq!(resp.status, "200 OK");
    assert_eq!(resp.content_type, "application/json");
    let body: Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body["state"], "done");
    assert_eq!(body["result"], 5);

    let Some(Reply::Stream(stream)) = jobs.route("GET", &format!("/jobs/{id}/events")) else {
        panic!("expected a stream");
    };
    let mut out = Vec::new();
    stream(&mut out).unwrap();
    assert!(String::from_utf8(out)
        .unwrap()
        .ends_with("event: done\ndata: 5\n\n"));

    for path in ["/jobs/missing", "/jobs/missing/events"] {
        let Some(Reply::Full(resp)) = jobs.route("GET", path) else {
            panic!("expected a full reply");
        };
        assert_eq!(resp.status, "404 Not Found");
    }
}

#[test]
fn other_requests_are_not_job_routes() {
    let jobs = Jobs::new();
    assert!(jobs.route("POST", "/jobs/x").is_none());
    assert!(jobs.route("GET", "/other").is_none());
    assert!(jobs.route("GET", "/jobs/").is_none());
    assert!(jobs.route("GET", "/jobs/a/b").is_none());
}

#[test]
fn progress_keeps_the_newest_lines_and_counts_the_dropped() {
    let jobs = Jobs::new();
    let id = jobs.spawn("chatty", |h| {
        for n in 1..=600 {
            h.progress(&format!("line {n}"));
        }
        Ok(json!(0))
    });
    let snap = settle(&jobs, &id);
    let lines = snap["progress"].as_array().unwrap();
    assert_eq!(lines.len(), 500);
    assert_eq!(lines[0], "line 101");
    assert_eq!(lines[499], "line 600");
    assert_eq!(snap["dropped"], 100);
}

#[test]
fn small_jobs_report_no_dropped_count() {
    let jobs = Jobs::new();
    let id = jobs.spawn("quiet", |h| {
        h.progress("only");
        Ok(json!(0))
    });
    assert!(settle(&jobs, &id).get("dropped").is_none());
}

/// A writer whose first write waits for a release, so the producer can outrun the stream.
struct Stalled {
    /// Signalled once the first write has started.
    started: mpsc::Sender<()>,
    /// Released by the test; `None` after the first write.
    gate: Option<mpsc::Receiver<()>>,
    /// Everything written.
    seen: Vec<u8>,
}

impl Write for Stalled {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            self.started.send(()).unwrap();
            gate.recv_timeout(PATIENCE).unwrap();
        }
        self.seen.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn slow_stream_skips_dropped_lines_and_still_ends_with_done() {
    let jobs = Jobs::new();
    let (id, release, step) = gated(&jobs);
    step.recv_timeout(PATIENCE).unwrap();
    let (started_tx, started) = mpsc::channel();
    let (open, gate) = mpsc::channel();
    let reader = {
        let jobs = jobs.clone();
        let id = id.clone();
        thread::spawn(move || {
            let mut out = Stalled {
                started: started_tx,
                gate: Some(gate),
                seen: Vec::new(),
            };
            jobs.stream(&id, &mut out).map(|()| out.seen)
        })
    };
    started.recv_timeout(PATIENCE).unwrap();
    let handle = JobHandle {
        shared: Arc::clone(&jobs.shared),
        id,
    };
    for n in 2..=600 {
        handle.progress(&format!("line {n}"));
    }
    open.send(()).unwrap();
    release.send(Ok(json!({"ok": true}))).unwrap();
    let text = String::from_utf8(reader.join().unwrap().unwrap()).unwrap();
    assert!(text.contains("earlier lines dropped"), "{text}");
    assert!(text.contains("data: \"line 600\"\n\n"), "{text}");
    assert_eq!(text.matches("data: \"line 600\"").count(), 1);
    assert!(
        text.ends_with("event: done\ndata: {\"ok\":true}\n\n"),
        "{text}"
    );
}
