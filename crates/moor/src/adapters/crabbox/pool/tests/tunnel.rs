//! A pool Box's local tunnel: the host port it forwards over the pool's own ssh words.

use std::sync::Arc;

use moor_client::link::lease::Verb;
use moor_core::seam::driver::{Driver as _, DriverError, TunnelFailure};
use moor_core::seam::proc::Tool;

use super::{handle, rig_on};
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{ChildReply, ScriptedProc};

/// A pool Box whose ssh child answers `replies` in order, the last one repeating.
fn tunnel_rig(replies: &[ChildReply]) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let (last, first) = replies.split_last().unwrap();
    for r in first {
        p.once_child(Tool::Ssh, &["-T"], r.clone());
    }
    p.on_child(Tool::Ssh, &["-T"], last.clone());
    let (_, cb) = rig_on(Arc::clone(&p));
    (p, cb)
}

fn refused() -> ChildReply {
    ChildReply::Exit {
        code: 255,
        stderr: "bind [127.0.0.1]:41000: Address already in use\n\
                 Could not request local forwarding."
            .to_owned(),
    }
}

fn ssh_spawns(p: &ScriptedProc) -> usize {
    p.calls().len()
}

#[test]
fn a_pool_local_tunnel_forwards_a_host_port_over_the_pool_words() {
    let (p, cb) = tunnel_rig(&[ChildReply::Ready("ready".to_owned())]);
    let t = cb.local_tunnel(&handle(), 8080).unwrap();
    let args = p.calls()[0].args.clone();
    let words = args.join(" ");
    assert_ne!(t.host_port, 0);
    let forward = format!("-L 127.0.0.1:{}:127.0.0.1:8080 ", t.host_port);
    for want in [
        "-T",
        "ExitOnForwardFailure=yes",
        forward.as_str(),
        "IdentitiesOnly=yes",
        "ProxyCommand=moor link-dial 42 moor-1a2b3c4d-9101",
        "crabbox@moor-1a2b3c4d-9101",
    ] {
        assert!(words.contains(want), "{want} in {words}");
    }
    assert!(t.tunnel.alive());
    let pid = t.tunnel.pid();
    drop(t);
    assert_eq!(p.dropped(), [pid]);
}

#[test]
fn after_a_relink_the_next_ssh_call_dials_the_new_runner_without_a_failure() {
    let p = Arc::new(ScriptedProc::new());
    p.on_child(Tool::Ssh, &["-T"], ChildReply::Ready("ready".to_owned()));
    let (host, cb) = rig_on(Arc::clone(&p));
    *host.runner.lock().unwrap() = Some("10.0.0.7:7701".to_owned());
    drop(cb.local_tunnel(&handle(), 8080).unwrap());
    *host.runner.lock().unwrap() = Some("10.0.0.8:7701".to_owned());
    drop(cb.local_tunnel(&handle(), 8081).unwrap());
    let calls = p.calls();
    assert_eq!(
        calls.len(),
        2,
        "one ssh per tunnel, none failed and retried"
    );
    for (call, runner) in calls.iter().zip(["10.0.0.7:7701", "10.0.0.8:7701"]) {
        let want = format!("ProxyCommand=moor link-dial --runner {runner} 42 moor-1a2b3c4d-9101");
        assert!(call.args.contains(&want), "{want} in {:?}", call.args);
    }
    let infos = host.verbs().iter().filter(|v| **v == Verb::Info).count();
    assert_eq!(
        infos, 1,
        "the words are reused with only the runner changed"
    );
}

#[test]
fn a_pool_local_tunnel_that_loses_its_port_tries_a_second_one() {
    let (p, cb) = tunnel_rig(&[refused(), ChildReply::Ready("ready".to_owned())]);
    let t = cb.local_tunnel(&handle(), 8080).unwrap();
    assert!(t.tunnel.alive());
    assert_eq!(ssh_spawns(&p), 2);
}

#[test]
fn a_pool_local_tunnel_that_cannot_forward_is_an_error_after_one_retry() {
    let (p, cb) = tunnel_rig(&[refused()]);
    let err = cb.local_tunnel(&handle(), 8080).unwrap_err();
    assert!(
        matches!(
            err,
            DriverError::Tunnel {
                failure: TunnelFailure::Exited {
                    code: Some(255),
                    ..
                },
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(ssh_spawns(&p), 2);
}

#[test]
fn a_pool_local_tunnel_failing_for_another_reason_is_not_retried() {
    for reply in [
        ChildReply::Exit {
            code: 255,
            stderr: "Permission denied (publickey).".to_owned(),
        },
        ChildReply::Hang,
        ChildReply::Ready("nothing like it".to_owned()),
    ] {
        let (p, cb) = tunnel_rig(&[reply]);
        assert!(cb.local_tunnel(&handle(), 8080).is_err());
        assert_eq!(ssh_spawns(&p), 1);
        assert_eq!(p.dropped().len(), 1);
    }
}
