//! A `run` through the controller socket: the client gets the command's output while it
//! runs, then the command's exit status.
use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use moor_core::{Request, Response};

use super::super::conn::serve;
use super::super::drain::Admitted;
use super::super::progress::report_while;
use crate::cli::{render, GlobalOpts};
use crate::client::{call, Target};
use crate::env::Env;
use crate::ops::run_fixture::{ready_with, req};
use crate::testing::driver::FakeDriver;
use crate::Moor;

const WAIT: Duration = Duration::from_secs(10);

/// Prints `first`, waits in the Box until `$HOME/release` exists, prints `second`, exits 3.
const COMMAND: &str =
    r#"echo first; until [ -e "$HOME/release" ]; do sleep 0.1; done; echo second; exit 3"#;

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
    let f = ready_with(FakeDriver::new());
    let log = f.dir.path().join("runs").join("1.log");
    let release = f.box_dir.path().join("home/release");
    let sock = f.dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let cx = Mutex::new(Some(f.cx));
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        serve(
            reader,
            stream,
            &Admitted::unwatched(),
            |id, request, (_, out)| match request {
                Request::Run(run) => {
                    let cx = cx.lock().unwrap().take().unwrap();
                    Some(report_while("run", id, WAIT, out, move |sink| {
                        Moor::from_cx(cx).run_streamed(&run, sink)
                    }))
                }
                other => Some(Response::ok(other.cmd(), None)),
            },
            |_, _, _, _, _| {},
        );
    });

    let (piece, got) = mpsc::channel();
    let client = thread::spawn(move || {
        call(
            &Env::default(),
            &Target::Socket(sock),
            &Request::Run(req(&[COMMAND])),
            |_| {},
            |data| piece.send(data.to_vec()).unwrap(),
        )
    });
    expect_output(&got, "first\n");
    std::fs::write(&release, "").unwrap();
    expect_output(&got, "second\n");
    let response = client.join().unwrap().unwrap();
    assert_eq!(render(&response, &GlobalOpts::default()).code, 3);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "first\nsecond\n",
        "the run log keeps the output"
    );
}
