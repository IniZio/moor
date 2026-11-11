use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Pipes;
use moor_core::{BoxName, ErrorKind, ForwardCloseReq, ForwardOpenReq, ForwardReq};
use serde_json::json;

use super::{close, connect, open};
use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::driver::FakeDriver;

fn target() -> moor_core::BoxTarget {
    BoxName::parse_allow_bare("t").unwrap().into()
}

fn open_req(port: u16) -> ForwardOpenReq {
    ForwardOpenReq {
        target: target(),
        port,
        local_port: 8080,
    }
}

/// The payloads of Box `f`'s events of `kind`, oldest first.
fn events(f: &Fx, kind: &str) -> Vec<serde_json::Value> {
    let mut rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.sort_by_key(|e| e.id);
    rows.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

/// Runs a connection to `port` that sends `sent`; what came back, or the refusal.
fn connect_with(f: &Fx, port: u16, sent: &[u8]) -> Result<Vec<u8>, crate::error::CommandError> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(Cursor::new(sent.to_vec())),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let req = ForwardReq {
        target: target(),
        port,
    };
    connect(&f.cx, &req, pipes).map(|_| out)
}

fn put_to_sleep(f: &Fx) {
    f.cx.store()
        .unwrap()
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::sleep(false))
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
}

#[test]
fn opening_a_forward_checks_the_port_and_records_it() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let view = open(&f.cx, &open_req(5177)).unwrap();
    assert_eq!(view.port, 5177);
    assert_eq!(
        events(&f, "port_forward_opened"),
        [json!({"port": 5177, "local_port": 8080})]
    );
}

#[test]
fn opening_a_forward_on_a_port_nothing_listens_on_is_refused_and_recorded() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let e = open(&f.cx, &open_req(9000)).unwrap_err();
    assert_eq!(e.error.to_string(), "nothing listens on port 9000 in t");
    assert!(events(&f, "port_forward_opened").is_empty());
    assert_eq!(
        events(&f, "port_forward_refused"),
        [json!({"port": 9000, "reason": "nothing listens on port 9000 in t"})]
    );
}

#[test]
fn a_box_not_on_a_node_cannot_forward() {
    let f = ready_with(FakeDriver::new());
    let e = open(&f.cx, &open_req(5177)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(
        e.error
            .to_string()
            .starts_with("port forward needs a Box on a node; t runs on "),
        "{e:?}"
    );
}

#[test]
fn a_node_the_controller_cannot_reach_is_said_not_to_be_linked() {
    let transport = DriverError::Transport {
        stderr_tail: "machine 3 is not linked".to_owned(),
    };
    let f = ready_with(
        FakeDriver::new()
            .on_a_node(&[5177])
            .failing_ports(transport),
    );
    let e = open(&f.cx, &open_req(5177)).unwrap_err();
    assert!(
        e.error.to_string().contains("is not linked to moor"),
        "{e:?}"
    );
}

#[test]
fn an_agent_refusal_names_the_port_and_the_box() {
    let cap = DriverError::Failed {
        summary: "already forwards 8 ports".to_owned(),
    };
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]).failing_ports(cap));
    assert_eq!(
        open(&f.cx, &open_req(5177)).unwrap_err().error.to_string(),
        "t already forwards 8 ports"
    );
    let tunnel = DriverError::Failed {
        summary: "ssh: connect to host failed".to_owned(),
    };
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]).failing_ports(tunnel));
    assert_eq!(
        open(&f.cx, &open_req(5177)).unwrap_err().error.to_string(),
        "cannot forward port 5177 in t: ssh: connect to host failed"
    );
}

#[test]
fn opening_a_forward_wakes_an_asleep_box() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    put_to_sleep(&f);
    open(&f.cx, &open_req(5177)).unwrap();
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    assert!(!row.asleep());
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
}

#[test]
fn a_connection_never_wakes_an_asleep_box() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    put_to_sleep(&f);
    let e = connect_with(&f, 5177, b"x").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(
        e.error.to_string(),
        "t is asleep; open the forward again to wake it"
    );
    assert!(f
        .cx
        .store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .asleep());
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("wake ")));
}

#[test]
fn a_connection_to_a_closed_box_says_the_forward_ended() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let req = moor_core::CloseReq {
        target: target(),
        force: true,
    };
    crate::ops::close::run(&f.cx, &req).unwrap();
    let e = connect_with(&f, 5177, b"x").unwrap_err();
    assert!(e
        .error
        .to_string()
        .ends_with("is closed; the forward ended"));
}

#[test]
fn a_connection_carries_bytes_both_ways_and_holds_the_box_awake() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    assert!(!f.cx.traffic().recent(f.id, f.cx.clock().now()));
    let back = connect_with(&f, 5177, b"GET / HTTP/1.1\r\n").unwrap();
    assert_eq!(back, b"GET / HTTP/1.1\r\n");
    assert_eq!(f.driver.port_bytes(), b"GET / HTTP/1.1\r\n");
    assert!(f.cx.traffic().recent(f.id, f.cx.clock().now()));
    assert!(
        events(&f, "port_forward_opened").is_empty(),
        "no event per connection"
    );
}

/// Stdin that yields nothing until it is told to end.
struct Quiet(std::sync::mpsc::Receiver<()>);

impl std::io::Read for Quiet {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        let _ = self.0.recv();
        Ok(0)
    }
}

#[test]
fn a_connection_with_no_byte_for_thirty_minutes_is_cut() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let (_hold, held) = std::sync::mpsc::channel();
    let (done, clock) = (AtomicBool::new(false), f.cx.clock());
    let cut = std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
                clock.sleep(Duration::from_mins(5));
            }
        });
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let pipes = Pipes {
            stdin: Box::new(Quiet(held)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        };
        let req = ForwardReq {
            target: target(),
            port: 5177,
        };
        let ended = connect(&f.cx, &req, pipes);
        done.store(true, Ordering::SeqCst);
        ended
    });
    assert!(cut.is_ok(), "{:?}", cut.err().map(|e| e.error));
}

#[test]
fn closing_a_forward_records_what_it_carried() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let req = ForwardCloseReq {
        target: target(),
        port: 5177,
        connections: 4,
        bytes_in: 10,
        bytes_out: 2048,
    };
    assert_eq!(close(&f.cx, &req).unwrap().port, 5177);
    assert_eq!(
        events(&f, "port_forward_closed"),
        [json!({"port": 5177, "connections": 4,
            "bytes_in": 10, "bytes_out": 2048})]
    );
}
