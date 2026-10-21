//! A `run` whose client is killed mid-run is cancelled and recorded interrupted.
use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::controller::{ClientFrame, PROTOCOL};
use moor_core::seam::driver::ExecOutcome;
use moor_core::{Request, Response};

use super::super::conn::serve;
use super::super::hangup::Hangup;
use super::super::progress::report_while;
use crate::env::Cancel;
use crate::ops::run_fixture::{ready_with, req};
use crate::testing::driver::FakeDriver;
use crate::Moor;

const WAIT: Duration = Duration::from_secs(10);

/// A client process holding the only end of a connection that asked for `request`.
fn client_process(socket: &std::path::Path, request: &Request) -> std::process::Child {
    let mut stream = UnixStream::connect(socket).unwrap();
    let hello = ClientFrame::Hello { protocol: PROTOCOL };
    stream.write_all(hello.to_line().as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let ask = ClientFrame::Request {
        id: "1".to_owned(),
        request: Box::new(request.clone()),
    };
    stream.write_all(ask.to_line().as_bytes()).unwrap();
    let mut holder = Command::new("sleep");
    holder.arg("30").stdin(Stdio::from(OwnedFd::from(stream)));
    holder.spawn().unwrap()
}

/// Serves one connection on `listener`, running its `run` over `cx` until the client hangs up
/// and passing the response to `answered`.
fn serve_one(
    listener: &UnixListener,
    cx: crate::cx::Cx,
    cancel: &Cancel,
    answered: &mpsc::Sender<Response>,
) {
    let (stream, _) = listener.accept().unwrap();
    let hangup = Hangup::of(&stream).unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    let cx = Mutex::new(Some(cx));
    serve(
        reader,
        stream,
        |id, request, out| match request {
            Request::Run(run) => {
                let cx = cx.lock().unwrap().take().unwrap();
                let response = hangup.cancels(cancel, || {
                    report_while("run", id, WAIT, out, move |sink| {
                        Moor::from_cx(cx).run_streamed(&run, sink)
                    })
                });
                answered.send(response.clone()).unwrap();
                response
            }
            other => Response::ok(other.cmd(), None),
        },
        |_, _, _, _| {},
    );
}

#[test]
fn a_run_whose_client_is_killed_is_cancelled_and_recorded_interrupted() {
    let f = ready_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]));
    let cancel = Cancel::default();
    let (started, running) = mpsc::channel();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let (until, seen) = (cancel.clone(), Arc::clone(&saw_cancel));
    let started = Mutex::new(started);
    f.driver.on_exec(move || {
        started.lock().unwrap().send(()).ok();
        for _ in 0..1000 {
            if until.is_cancelled() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        seen.store(until.is_cancelled(), Ordering::SeqCst);
    });
    let sock = f.dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (answered, answer) = mpsc::channel();
    let server = thread::spawn(move || serve_one(&listener, f.cx, &cancel, &answered));
    let mut client = client_process(&sock, &Request::Run(req(&["sleep", "600"])));
    running.recv_timeout(WAIT).expect("the command started");
    client.kill().unwrap();
    client.wait().unwrap();
    let response = answer.recv_timeout(WAIT).expect("the run ended");
    server.join().unwrap();
    assert!(saw_cancel.load(Ordering::SeqCst), "the run was cancelled");
    let value = response.value.expect("a run value");
    assert_eq!(value.get("interrupted"), Some(&true.into()), "{value}");
    let store = crate::store::Store::open(
        &f.dir.path().join(crate::store::STORE_FILE),
        Arc::new(crate::testing::clock::FakeClock::new(
            moor_core::seam::clock::Ts::from_millis(0),
        )),
    )
    .unwrap();
    let name = moor_core::BoxName::parse_allow_bare("t").unwrap();
    let id = store.latest_box_by_name(&name).unwrap().unwrap().id;
    assert!(store.last_run(id).unwrap().unwrap().interrupted);
}
