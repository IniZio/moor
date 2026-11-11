//! Ports of a pool Box go to its own host, and what the host says becomes the person's message.
#![allow(clippy::unwrap_used, reason = "tests")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::reach::StdConn;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, Forwards, NO_PORTS, OLD_AGENT_PORTS};
use moor_core::seam::proc::Pipes;

use super::super::forward::PoolPorts;
use super::PoolHosts;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::ScriptedProc;

/// A host whose port calls answer as scripted and record the machine, lease and port asked.
#[derive(Default)]
struct Host {
    listens: Mutex<Option<Result<bool, String>>>,
    asked: Mutex<Vec<(i64, String, u16)>>,
}

impl PoolHosts for Host {
    fn lease(
        &self,
        _machine: i64,
        _verb: Verb,
        _slug: Option<&str>,
        _key: Option<&str>,
        _boot: &Boot,
    ) -> Result<Answer, String> {
        Ok(Answer::Done)
    }

    fn dial_command(&self, _machine: i64, _slug: &str) -> Result<String, String> {
        Ok(String::new())
    }
}

impl PoolPorts for Host {
    fn port_listens(&self, machine: i64, slug: &str, port: u16) -> Result<bool, String> {
        self.asked
            .lock()
            .unwrap()
            .push((machine, slug.to_owned(), port));
        self.listens.lock().unwrap().clone().unwrap()
    }

    fn port_conn(&self, machine: i64, slug: &str, port: u16) -> Result<StdConn, String> {
        self.asked
            .lock()
            .unwrap()
            .push((machine, slug.to_owned(), port));
        let scripted = self.listens.lock().unwrap().clone();
        if let Some(Err(e)) = scripted {
            return Err(e);
        }
        let (near, mut far) = UnixStream::pair().unwrap();
        // The Box's app: answers each chunk upper-cased.
        thread::spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok(n @ 1..) = far.read(&mut buf) {
                let up: Vec<u8> = buf[..n].to_ascii_uppercase();
                if far.write_all(&up).is_err() {
                    return;
                }
            }
        });
        Ok(StdConn::from(near))
    }
}

fn rig(listens: Result<bool, String>) -> (Arc<Host>, Crabbox) {
    let host = Arc::new(Host::default());
    *host.listens.lock().unwrap() = Some(listens);
    let cb = Crabbox::fake(Arc::new(ScriptedProc::new()))
        .with_pool_hosts(Some(Arc::clone(&host) as Arc<dyn PoolHosts>))
        .with_pool_ports(Some(Arc::clone(&host) as Arc<dyn PoolPorts>));
    (host, cb)
}

fn handle() -> BoxHandle {
    BoxHandle::new("external@hv/42/moor-1a2b3c4d-9101")
}

#[test]
fn a_probe_and_a_connection_go_to_the_lease_on_its_own_machine() {
    let (host, cb) = rig(Ok(true));
    assert_eq!(cb.port_listens(&handle(), 5177), Ok(true));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(std::io::Cursor::new(b"hello".to_vec())),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    cb.forward_piped(&handle(), 5177, pipes).unwrap();
    assert_eq!(out, b"HELLO");
    let want = (42, "moor-1a2b3c4d-9101".to_owned(), 5177);
    assert_eq!(*host.asked.lock().unwrap(), [want.clone(), want]);
}

#[test]
fn a_box_that_is_not_on_a_node_is_unsupported() {
    let (_, cb) = rig(Ok(true));
    let other = BoxHandle::new("p/moor-9-abc");
    assert_eq!(
        cb.port_listens(&other, 5177),
        Err(DriverError::Unsupported(NO_PORTS))
    );
}

#[test]
fn an_agent_that_predates_ports_is_told_apart_from_a_refusal_and_a_lost_link() {
    let old = "the links broker refused: the broker carries one lease, dial or status: not \
               understood: unknown variant `port`";
    let (_, cb) = rig(Err(old.to_owned()));
    assert_eq!(
        cb.port_listens(&handle(), 1),
        Err(DriverError::Unsupported(OLD_AGENT_PORTS))
    );
    let (_, cb) = rig(Err("machine 42 is not linked".to_owned()));
    assert!(matches!(
        cb.port_listens(&handle(), 1),
        Err(DriverError::Transport { .. })
    ));
    let (_, cb) = rig(Err(
        "machine 42 refused: already forwards 8 ports".to_owned()
    ));
    assert_eq!(
        cb.port_listens(&handle(), 1),
        Err(DriverError::Failed {
            summary: "already forwards 8 ports".to_owned()
        })
    );
}

/// Stdin that yields nothing until its sender hangs up, as a browser tab that stays open.
struct Quiet(std::sync::mpsc::Receiver<()>);

impl Read for Quiet {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        let _ = self.0.recv();
        Ok(0)
    }
}

#[test]
fn the_pump_cuts_a_quiet_connection_once_its_caller_says_stop() {
    let (_, cb) = rig(Ok(true));
    let (_hold, held) = std::sync::mpsc::channel();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = thread::scope(|s| {
        s.spawn(|| {
            thread::sleep(std::time::Duration::from_millis(300));
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let pipes = Pipes {
            stdin: Box::new(Quiet(held)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| stop.load(std::sync::atomic::Ordering::SeqCst),
        };
        cb.forward_piped(&handle(), 5177, pipes)
    });
    assert_eq!(ended, Ok(()));
}

#[test]
fn the_pump_with_stop_already_set_ends_at_once() {
    let (_, cb) = rig(Ok(true));
    let (_hold, held) = std::sync::mpsc::channel();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(Quiet(held)),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| true,
    };
    assert_eq!(cb.forward_piped(&handle(), 5177, pipes), Ok(()));
}
