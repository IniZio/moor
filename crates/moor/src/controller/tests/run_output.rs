//! A `run` through the controller socket: the client gets the command's output while it
//! runs, then the command's exit status.
use std::io::{BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use moor_core::seam::driver::ExecOutcome;
use moor_core::{Request, Response};

use super::super::conn::serve;
use super::super::progress::report_while;
use crate::cli::{render, GlobalOpts};
use crate::client::{call, Target};
use crate::env::Env;
use crate::ops::run_fixture::{ready_with, req};
use crate::testing::driver::FakeDriver;
use crate::Moor;

const WAIT: Duration = Duration::from_secs(10);

/// Appends `text` to `log` once the command has written it, after `release` fires.
fn hold_then_append(log: PathBuf, release: Receiver<()>, text: &'static str) -> impl Fn() {
    let release = Mutex::new(release);
    let once = AtomicBool::new(false);
    move || {
        if !log.exists() || once.swap(true, Ordering::SeqCst) {
            return;
        }
        release
            .lock()
            .unwrap()
            .recv_timeout(WAIT)
            .expect("the test released the command");
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }
}

/// Collects pieces from `got` until they spell `want`.
fn expect_output(got: &Receiver<Vec<u8>>, want: &str) {
    let mut seen = Vec::new();
    while seen != want.as_bytes() {
        let piece = got.recv_timeout(WAIT);
        assert!(piece.is_ok(), "waited for {want:?}, saw {seen:?}");
        seen.extend(piece.unwrap_or_default());
    }
}

#[test]
fn run_output_reaches_the_client_while_the_command_runs() {
    let f = ready_with(
        FakeDriver::new()
            .exec_results([Ok(ExecOutcome::Exited { code: 3 })])
            .exec_stdout(["first\n"]),
    );
    let (release, held) = mpsc::channel();
    let log = f.dir.path().join("runs").join("1.log");
    f.driver
        .after_exec(hold_then_append(log.clone(), held, "second\n"));
    let sock = f.dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let cx = Mutex::new(Some(f.cx));
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        serve(
            reader,
            stream,
            |id, request, out| match request {
                Request::Run(run) => {
                    let cx = cx.lock().unwrap().take().unwrap();
                    report_while("run", id, WAIT, out, move |sink| {
                        Moor::from_cx(cx).run_streamed(&run, sink)
                    })
                }
                other => Response::ok(other.cmd(), None),
            },
            |_, _, _, _| {},
        );
    });

    let (piece, got) = mpsc::channel();
    let client = thread::spawn(move || {
        call(
            &Env::default(),
            &Target::Socket(sock),
            &Request::Run(req(&["make", "test"])),
            |_| {},
            |data| piece.send(data.to_vec()).unwrap(),
        )
    });
    expect_output(&got, "first\n");
    release.send(()).unwrap();
    expect_output(&got, "second\n");
    let response = client.join().unwrap().unwrap();
    assert_eq!(render(&response, &GlobalOpts::default()).code, 3);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "first\nsecond\n",
        "the run log keeps the output"
    );
}
