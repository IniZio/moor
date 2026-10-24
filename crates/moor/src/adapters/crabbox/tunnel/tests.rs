use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{Driver, DriverError, TunnelFailure};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::Crabbox;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{ChildReply, Reply, ScriptedProc};
use crate::testing::rng::FakeRng;

fn fixture(reply: ChildReply) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    p.on_child(Tool::Crabbox, &["tunnel"], reply);
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn open(
    c: &Crabbox,
    handle: &str,
) -> Result<Box<dyn moor_core::seam::driver::TunnelHandle>, DriverError> {
    c.reverse_tunnel(&BoxHandle::new(handle), 8080, Path::new(SOCKET))
}

const SOCKET: &str = "/tmp/moor-1000/px-0123456789abcdef";

fn ready() -> ChildReply {
    ChildReply::Ready(format!(r#"{{"box":8080,"socket":"{SOCKET}"}}"#))
}

#[test]
fn argv_and_env_are_recorded() {
    let (p, c) = fixture(ready());
    let _t = open(&c, "sprites/moor-t7").unwrap();
    let calls = p.calls();
    assert_eq!(calls.len(), 1);
    let call = calls.first().unwrap();
    assert_eq!(
        call.args,
        [
            "tunnel",
            "--provider",
            "sprites",
            "--id",
            "moor-t7",
            "--reverse",
            "8080:/tmp/moor-1000/px-0123456789abcdef",
            "--json"
        ]
    );
    let root = ("CRABBOX_EXTERNAL_WORK_ROOT".to_owned(), "moor".to_owned());
    assert!(call.env.iter().any(|(k, _)| k == "GIT_CEILING_DIRECTORIES"));
    assert!(call
        .env
        .iter()
        .all(|e| e.0 == "GIT_CEILING_DIRECTORIES" || *e == root));
}

#[test]
fn static_provider_gets_its_env() {
    let (p, c) = fixture(ready());
    let _t = open(&c, "ssh/box1").unwrap();
    let calls = p.calls();
    let env = &calls.first().unwrap().env;
    assert!(env.contains(&("CRABBOX_STATIC_ID".to_owned(), "box1".to_owned())));
    assert!(env.contains(&("CRABBOX_STATIC_PORT".to_owned(), "22".to_owned())));
}

#[test]
fn ready_line_gives_live_handle_that_kills_on_drop() {
    let (p, c) = fixture(ready());
    let t = open(&c, "sprites/moor-t7").unwrap();
    let pid = t.pid();
    assert!(t.alive());
    assert!(p.dropped().is_empty());
    drop(t);
    assert_eq!(p.dropped(), vec![pid]);
}

#[test]
fn exited_child_is_not_alive() {
    let (_p, c) = fixture(ChildReply::ReadyThenExit {
        line: format!(r#"{{"box":8080,"socket":"{SOCKET}"}}"#),
        code: 1,
    });
    let t = open(&c, "sprites/moor-t7").unwrap();
    assert!(!t.alive());
}

#[test]
fn pid_probe_does_not_decide_liveness() {
    let (p, c) = fixture(ready());
    let t = open(&c, "sprites/moor-t7").unwrap();
    p.dead(t.pid());
    assert!(t.alive());
}

#[test]
fn other_ends_in_the_ready_line_fail_and_kill_the_child() {
    for line in [
        format!(r#"{{"box":8081,"socket":"{SOCKET}"}}"#),
        r#"{"box":8080,"socket":"/tmp/moor-1000/px-other"}"#.to_owned(),
        r#"{"box":8080,"host":9090}"#.to_owned(),
    ] {
        let (p, c) = fixture(ChildReply::Ready(line));
        let err = open(&c, "sprites/moor-t7").err().unwrap();
        assert!(matches!(
            err,
            DriverError::Failed { ref summary } if summary == "reverse tunnel printed an unexpected ready line"
        ));
        assert_eq!(p.dropped().len(), 1);
    }
}

#[test]
fn unparsable_ready_line_fails_without_echo() {
    let (p, c) = fixture(ChildReply::Ready("box 127.0.0.1:8080 -> secret".to_owned()));
    let err = open(&c, "sprites/moor-t7").err().unwrap();
    assert!(matches!(
        err,
        DriverError::Failed { ref summary } if !summary.contains("secret")
    ));
    assert_eq!(p.dropped().len(), 1);
}

#[test]
fn early_exit_keeps_code_and_stderr() {
    let (_p, c) = fixture(ChildReply::Exit {
        code: 5,
        stderr: "box port never opened".to_owned(),
    });
    let err = open(&c, "sprites/moor-t7").err().unwrap();
    assert!(matches!(
        err,
        DriverError::Tunnel {
            failure: TunnelFailure::Exited { code: Some(5), signal: None },
            ref stderr_tail,
        } if stderr_tail == "box port never opened"
    ));
}

#[test]
fn silent_child_times_out_and_is_killed() {
    let (p, c) = fixture(ChildReply::Hang);
    let err = open(&c, "sprites/moor-t7").err().unwrap();
    assert!(matches!(
        err,
        DriverError::Tunnel {
            failure: TunnelFailure::TimedOut(d),
            ..
        } if d == Duration::from_secs(30)
    ));
    assert_eq!(p.dropped().len(), 1);
}

#[test]
fn unspawnable_crabbox_is_unavailable() {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p);
    let err = open(&c, "sprites/moor-t7").err().unwrap();
    assert!(matches!(err, DriverError::Unavailable(_)));
}

const RACE: &str = "lease cbx_1 claim changed; retry";

fn race() -> ChildReply {
    ChildReply::Exit {
        code: 2,
        stderr: RACE.to_owned(),
    }
}

/// A crabbox over `p` whose clock the test reads.
fn timed(p: &Arc<ScriptedProc>) -> (Crabbox, Arc<FakeClock>) {
    p.on(
        Tool::Crabbox,
        &["--version"],
        Reply::Ok("crabbox 0.79.1-moor.6\n".into()),
    );
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let c =
        Crabbox::new(p.clone(), clock.clone(), Arc::new(FakeRng::default())).with_scratch_leases();
    (c, clock)
}

fn tunnel_calls(p: &ScriptedProc) -> usize {
    p.calls()
        .iter()
        .filter(|c| c.args.first().is_some_and(|a| a == "tunnel"))
        .count()
}

#[test]
fn a_tunnel_that_loses_the_claim_race_to_a_run_is_opened_again() {
    let p = Arc::new(ScriptedProc::new());
    p.once_child(Tool::Crabbox, &["tunnel"], race());
    p.on_child(Tool::Crabbox, &["tunnel"], ready());
    let (c, clock) = timed(&p);
    let t = open(&c, "sprites/moor-t7").unwrap();
    assert!(t.alive());
    assert_eq!(tunnel_calls(&p), 2);
    assert_eq!(clock.now(), Ts::from_millis(1000));
}

#[test]
fn a_tunnel_that_keeps_losing_the_claim_race_gives_up_with_the_race() {
    let p = Arc::new(ScriptedProc::new());
    p.on_child(Tool::Crabbox, &["tunnel"], race());
    let (c, clock) = timed(&p);
    let err = open(&c, "sprites/moor-t7").err().unwrap();
    assert!(matches!(
        err,
        DriverError::Tunnel {
            failure: TunnelFailure::Exited { code: Some(2), signal: None },
            ref stderr_tail,
        } if stderr_tail == RACE
    ));
    assert_eq!(tunnel_calls(&p), 16);
    assert_eq!(clock.now(), Ts::from_millis(15_000));
}

#[test]
fn other_tunnel_failures_are_not_retried() {
    for reply in [
        ChildReply::Exit {
            code: 2,
            stderr: "unknown flag --reverse".to_owned(),
        },
        ChildReply::Exit {
            code: 1,
            stderr: RACE.to_owned(),
        },
        ChildReply::Hang,
        ChildReply::Ready("not json".to_owned()),
    ] {
        let p = Arc::new(ScriptedProc::new());
        p.on_child(Tool::Crabbox, &["tunnel"], reply);
        let (c, clock) = timed(&p);
        open(&c, "sprites/moor-t7").err().unwrap();
        assert_eq!(tunnel_calls(&p), 1);
        assert_eq!(clock.now(), Ts::from_millis(0));
    }
}
