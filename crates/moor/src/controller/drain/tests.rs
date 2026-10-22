//! The drain: in-flight requests finish, waits and idle connections end, and nothing new is
//! accepted. The SIGTERM test runs the drain in a child process and signals only that child.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::exit,
    reason = "test assertions index decoded frames and fail loudly; the child exits as the controller does"
)]

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::controller::PROTOCOL;
use moor_core::seam::proc::{Deadline, Output, Proc, ProcSpec, Tool};
use moor_core::{Request, Response};
use serde_json::{json, Value};

use super::{on_sigterm, serve_until, Admitted, Drain, Handler};
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::controller::conn::serve;
use crate::controller::progress::{report_while, wait_unless_draining};
use crate::controller::Sink;
use crate::env::Env;

const WAIT: Duration = Duration::from_secs(10);

/// A progress interval no test outlasts, so a reply never waits behind progress frames.
const QUIET: Duration = Duration::from_secs(600);

/// How each request kind is answered by the fake engine.
type Answer = Arc<dyn Fn(&str, Request, &mut dyn Write, &Drain) -> Option<Response> + Send + Sync>;

/// A handler that serves frames with `answer` and no watch.
fn handler(answer: Answer) -> Handler {
    Arc::new(move |stream: UnixStream, conn: Admitted| {
        let reader = BufReader::new(stream.try_clone().expect("clone"));
        let answer = Arc::clone(&answer);
        serve(
            reader,
            stream,
            &conn,
            |id, r, out| answer(id, r, out, conn.drain()),
            |_, _, _, _| {},
        );
    })
}

/// A sink nobody reads.
fn quiet() -> Sink {
    Arc::new(Mutex::new(io::sink()))
}

/// A served socket and the means to start its drain.
struct Served {
    socket: PathBuf,
    drain: Arc<Drain>,
    /// Writing here starts the drain, as SIGTERM does.
    signal: UnixStream,
    /// How many connections the drain cut off, once it returns.
    returned: Receiver<usize>,
}

impl Served {
    fn start(dir: &Path, answer: Answer, timeout: Duration) -> Self {
        let socket = dir.join("c.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let (wake, signal) = UnixStream::pair().expect("pair");
        let drain = Arc::new(Drain::default());
        let (tx, returned) = mpsc::channel();
        let serving = Arc::clone(&drain);
        thread::spawn(move || {
            let listeners = [(listener, handler(answer))];
            let left = serve_until(&listeners, &wake, &serving, timeout, &quiet());
            tx.send(left).ok();
            drop(listeners);
        });
        Self {
            socket,
            drain,
            signal,
            returned,
        }
    }

    fn sigterm(&mut self) {
        self.signal.write_all(&[1]).expect("wake");
        for _ in 0..500 {
            if self.drain.begun() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the drain never began");
    }
}

/// One client connection.
struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn connect(socket: &Path) -> Self {
        let writer = UnixStream::connect(socket).expect("connect");
        writer.set_read_timeout(Some(WAIT)).expect("timeout");
        let reader = BufReader::new(writer.try_clone().expect("clone"));
        let mut this = Self { reader, writer };
        this.send(&json!({"frame": "hello", "protocol": PROTOCOL}));
        assert_eq!(this.frame()["frame"], "hello");
        this
    }

    fn send(&mut self, frame: &Value) {
        writeln!(self.writer, "{frame}").expect("send");
    }

    fn request(&mut self, id: &str, request: &Value) {
        self.send(&json!({"frame": "request", "id": id, "request": request}));
    }

    fn frame(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }

    /// The response or error frame for `id`, skipping progress frames.
    fn reply(&mut self, id: &str) -> Value {
        loop {
            let frame = self.frame();
            if matches!(frame["frame"].as_str(), Some("response" | "error")) {
                assert_eq!(frame["id"], id, "{frame}");
                return frame;
            }
        }
    }

    /// Whether the controller closed the connection.
    fn closed(&mut self) -> bool {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => true,
            Err(e) => e.kind() == io::ErrorKind::ConnectionReset,
            Ok(_) => false,
        }
    }
}

/// Requests that tell the fake engine they began and wait for `release` to answer.
fn held(began: Sender<String>, release: Receiver<()>) -> Answer {
    let release = Mutex::new(release);
    Arc::new(move |id, request, out, _| {
        let cmd = request.cmd();
        began.send(cmd.to_owned()).ok();
        let release = &release;
        Some(report_while(cmd, id, WAIT, out, move |_| {
            release
                .lock()
                .expect("lock")
                .recv_timeout(WAIT)
                .expect("released");
            Response::ok(cmd, None)
        }))
    })
}

fn start_request() -> Value {
    json!({"cmd": "start", "name": "demo"})
}

#[test]
fn requests_in_flight_are_answered_before_the_drain_returns() {
    let dir = tempfile::tempdir().expect("tmp");
    let (began, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut served = Served::start(dir.path(), held(began, released), WAIT);
    let mut quick = Client::connect(&served.socket);
    let mut start = Client::connect(&served.socket);
    quick.request("q", &json!({"cmd": "show"}));
    start.request("s", &start_request());
    let mut seen = [
        started.recv_timeout(WAIT).expect("began"),
        started.recv_timeout(WAIT).expect("began"),
    ];
    seen.sort();
    assert_eq!(seen, ["show", "start"]);
    served.sigterm();
    thread::sleep(Duration::from_millis(200));
    assert!(
        served.returned.try_recv().is_err(),
        "the drain returned while two requests were in flight"
    );
    release.send(()).expect("release");
    release.send(()).expect("release");
    assert_eq!(served.returned.recv_timeout(WAIT).expect("drained"), 0);
    for (client, id, cmd) in [(&mut quick, "q", "show"), (&mut start, "s", "start")] {
        let reply = client.reply(id);
        assert_eq!(reply["frame"], "response", "{reply}");
        assert_eq!(reply["envelope"]["cmd"], cmd);
        assert_eq!(reply["envelope"]["class"], "ok");
        assert!(client.closed(), "{id} stayed open after the drain");
    }
}

#[test]
fn a_wait_in_flight_gets_restarting_and_the_connection_closes() {
    let dir = tempfile::tempdir().expect("tmp");
    let (began, waiting) = mpsc::channel();
    let (never, forever) = mpsc::channel::<()>();
    let forever = Arc::new(Mutex::new(forever));
    let answer: Answer = Arc::new(move |id, request, out, drain| {
        let forever = Arc::clone(&forever);
        began.send(()).ok();
        wait_unless_draining(request.cmd(), id, QUIET, out, drain, move || {
            forever.lock().expect("lock").recv().ok();
            Response::ok("wait", None)
        })
    });
    let mut served = Served::start(dir.path(), answer, WAIT);
    let mut client = Client::connect(&served.socket);
    client.request("w", &json!({"cmd": "show"}));
    waiting.recv_timeout(WAIT).expect("the wait began");
    served.sigterm();
    let reply = client.reply("w");
    assert_eq!(reply["frame"], "error", "{reply}");
    assert_eq!(reply["kind"], "restarting", "{reply}");
    assert!(
        client.closed(),
        "the connection stayed open after restarting"
    );
    assert_eq!(served.returned.recv_timeout(WAIT).expect("drained"), 0);
    drop(never);
}

#[test]
fn an_idle_connection_does_not_hold_the_drain_open() {
    let dir = tempfile::tempdir().expect("tmp");
    let (began, _) = mpsc::channel();
    let (_release, released) = mpsc::channel();
    let mut served = Served::start(dir.path(), held(began, released), WAIT);
    let mut idle = Client::connect(&served.socket);
    served.sigterm();
    assert_eq!(
        served
            .returned
            .recv_timeout(WAIT / 2)
            .expect("the drain waited on an idle connection"),
        0
    );
    assert!(idle.closed(), "the idle connection stayed open");
}

#[test]
fn a_connect_after_the_drain_began_waits_unaccepted_then_fails() {
    let dir = tempfile::tempdir().expect("tmp");
    let (began, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut served = Served::start(dir.path(), held(began, released), WAIT);
    let mut busy = Client::connect(&served.socket);
    busy.request("s", &start_request());
    started.recv_timeout(WAIT).expect("began");
    served.sigterm();
    let late = UnixStream::connect(&served.socket).expect("a connect queues in the backlog");
    late.set_read_timeout(Some(Duration::from_millis(300)))
        .expect("timeout");
    writeln!(&late, "{}", json!({"frame": "hello", "protocol": PROTOCOL})).expect("send");
    let mut reader = BufReader::new(late);
    let mut line = String::new();
    let unanswered = reader.read_line(&mut line);
    assert!(
        matches!(&unanswered, Err(e) if e.kind() == io::ErrorKind::WouldBlock),
        "a connect after the drain began was served: {unanswered:?} {line:?}"
    );
    release.send(()).expect("release");
    assert_eq!(busy.reply("s")["frame"], "response");
    assert_eq!(served.returned.recv_timeout(WAIT).expect("drained"), 0);
    reader
        .get_ref()
        .set_read_timeout(Some(WAIT))
        .expect("timeout");
    let mut client = Client {
        writer: reader.get_ref().try_clone().expect("clone"),
        reader,
    };
    assert!(client.closed(), "the late connect outlived the listener");
}

#[test]
fn the_drain_returns_at_its_timeout_with_the_requests_still_in_flight() {
    let dir = tempfile::tempdir().expect("tmp");
    let (began, started) = mpsc::channel();
    let (_release, released) = mpsc::channel();
    let mut served = Served::start(
        dir.path(),
        held(began, released),
        Duration::from_millis(200),
    );
    let mut busy = Client::connect(&served.socket);
    busy.request("s", &start_request());
    started.recv_timeout(WAIT).expect("began");
    served.sigterm();
    assert_eq!(served.returned.recv_timeout(WAIT).expect("drained"), 1);
}

/// Set in the child process the SIGTERM test starts; names its working dir.
const CHILD: &str = "MOOR_DRAIN_TEST_CHILD";

/// The child's streamed command: it marks that it began, outlives the signal, and prints.
const SCRIPT: &str = "touch began; sleep 1; echo finished";

/// In the child: serves runs that stream [`SCRIPT`] through the real spawner until SIGTERM,
/// drains, and exits 0 as the controller does.
#[test]
fn sigterm_child() {
    let Some(dir) = std::env::var_os(CHILD).map(PathBuf::from) else {
        return;
    };
    let wake = on_sigterm().expect("take SIGTERM");
    let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
    let tools = ToolPaths {
        crabbox: Some(PathBuf::from("/bin/sh")),
        ..ToolPaths::default()
    };
    let proc = Arc::new(SystemProc::new(&env, &tools));
    let cwd = dir.clone();
    let answer: Answer = Arc::new(move |id, _, out, _| {
        let proc = Arc::clone(&proc);
        let cwd = cwd.clone();
        Some(report_while("run", id, WAIT, out, move |_| {
            let mut spec = ProcSpec::new(Tool::Crabbox, Deadline(WAIT)).args(["-c", SCRIPT]);
            spec.cwd = Some(cwd.clone());
            match proc.run_streamed(&spec, &Output::Log(cwd.join("run.log"))) {
                Ok(()) => Response::ok("run", None),
                Err(e) => Response::ok("run", Some(json!({"error": e.to_string()}))),
            }
        }))
    });
    let listener = UnixListener::bind(dir.join("c.sock")).expect("bind");
    let drain = Arc::new(Drain::default());
    let listeners = [(listener, handler(answer))];
    let left = serve_until(&listeners, &wake, &drain, WAIT, &quiet());
    std::process::exit(i32::from(left != 0));
}

#[test]
fn sigterm_lets_a_streamed_run_finish_and_exits_zero() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "controller::drain::tests::sigterm_child",
            "--nocapture",
        ])
        .env(CHILD, dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn child");
    let socket = dir.path().join("c.sock");
    let connected = (0..500).find_map(|_| {
        thread::sleep(Duration::from_millis(10));
        UnixStream::connect(&socket).ok()
    });
    drop(connected.expect("the child never listened"));
    let mut client = Client::connect(&socket);
    client.request("r", &json!({"cmd": "run", "box": "demo", "argv": ["true"]}));
    let marker = dir.path().join("began");
    assert!(
        (0..500).any(|_| {
            thread::sleep(Duration::from_millis(10));
            marker.exists()
        }),
        "the streamed command never began"
    );
    let pid = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).expect("SIGTERM the child");
    let reply = client.reply("r");
    assert_eq!(reply["frame"], "response", "{reply}");
    assert_eq!(
        reply["envelope"]["value"],
        Value::Null,
        "the run was cut short: {reply}"
    );
    let log = std::fs::read_to_string(dir.path().join("run.log")).expect("run log");
    assert_eq!(log, "finished\n");
    let status = (0..1000).find_map(|_| {
        let done = child.try_wait().expect("try_wait");
        if done.is_none() {
            thread::sleep(Duration::from_millis(10));
        }
        done
    });
    let status = status.unwrap_or_else(|| {
        child.kill().ok();
        panic!("the child kept running after its drain");
    });
    assert_eq!(status.code(), Some(0), "{status:?}");
}
