//! The front socket: where it lives, who may open it, and how a running tunnel keeps reaching it.

use std::fs::{self, Permissions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxId, Secret};

use super::{start, working_box, Stop, World, LEASE, TICK};
use crate::proxy::detached::{read_spec, write_spec};
use crate::store::Lease;
use crate::testing::bridge::bridge;
use crate::testing::rng::FakeRng;

/// How the front answers an origin-form request; nono never sees one.
pub(super) const REFUSAL: &str = "HTTP/1.1 403 Forbidden\r\n";

/// Sends an origin-form request into the Box port `port` and returns the reply.
pub(super) fn ask(port: u16) -> String {
    let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = tcp.write_all(b"GET /x HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut reply = String::new();
    let _ = tcp.read_to_string(&mut reply);
    reply
}

#[test]
fn the_front_listens_on_a_0600_socket_in_a_0700_dir_of_this_user() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    let socket = run.socket();
    assert_eq!(socket.parent(), Some(w.sockets.as_path()));
    let dir = fs::symlink_metadata(&w.sockets).unwrap();
    assert!(dir.is_dir());
    assert_eq!((dir.uid(), dir.mode() & 0o777), (w.owner, 0o700));
    let file = fs::symlink_metadata(socket).unwrap();
    assert!(file.file_type().is_socket());
    assert_eq!((file.uid(), file.mode() & 0o777), (w.owner, 0o600));
    assert!(ask(bridge(socket)).starts_with(REFUSAL));
}

#[test]
fn the_tunnel_is_requested_with_the_front_socket() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    let calls = w.driver.tunnels();
    let call = calls.first().unwrap();
    assert_eq!(call.socket, run.socket());
    assert!(ask(bridge(&call.socket)).starts_with(REFUSAL));
}

/// Starts the supervisor in `w` and expects the socket directory check to refuse it.
fn assert_refused(w: &World) {
    let deps = w.deps();
    assert_eq!(start(&deps, w.id).err(), Some(1));
    let err = w.stderr();
    assert!(
        err.contains("is not a directory that only this user can enter"),
        "{err}"
    );
    assert!(w.driver.tunnels().is_empty());
    assert_eq!(w.row().lease_owner, None);
    assert_eq!(fs::read_dir(&w.sockets).unwrap().count(), 0);
}

#[test]
fn the_proxy_refuses_a_socket_dir_of_another_user() {
    let mut w = World::new([]);
    w.owner = w.owner.checked_add(1).unwrap();
    assert_refused(&w);
}

#[test]
fn the_proxy_refuses_a_socket_dir_others_can_enter() {
    let w = World::new([]);
    fs::create_dir(&w.sockets).unwrap();
    fs::set_permissions(&w.sockets, Permissions::from_mode(0o755)).unwrap();
    assert_refused(&w);
}

#[test]
fn a_restarted_proxy_listens_on_the_same_socket_so_the_open_tunnel_reaches_it() {
    let w = World::new([]);
    let deps = w.deps();
    let first = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let tunnel = bridge(&w.driver.tunnels().first().unwrap().socket);
    assert_eq!(first.finish(Stop::Failed), 1);
    assert!(ask(tunnel).is_empty());

    let second = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    let calls = w.driver.tunnels();
    let (old, new) = (calls.first().unwrap(), calls.last().unwrap());
    assert_eq!((&new.socket, new.box_port), (&old.socket, old.box_port));
    assert_eq!(second.socket(), old.socket);
    assert!(ask(tunnel).starts_with(REFUSAL));
}

#[test]
fn the_tick_binds_a_removed_socket_again_and_the_open_tunnel_reaches_it() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let socket = run.socket().to_path_buf();
    let tunnel = bridge(&socket);
    fs::remove_file(&socket).unwrap();
    assert!(ask(tunnel).is_empty());

    w.clock.sleep(TICK);
    assert!(run.tick().is_none());

    assert_eq!(fs::symlink_metadata(&socket).unwrap().mode() & 0o777, 0o600);
    assert!(ask(tunnel).starts_with(REFUSAL));
    assert_eq!(w.driver.tunnels().len(), 1);
}

#[test]
fn a_front_that_cannot_be_bound_again_stops_the_proxy() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    fs::remove_file(run.socket()).unwrap();
    fs::set_permissions(&w.sockets, Permissions::from_mode(0o755)).unwrap();

    assert_eq!(run.tick(), Some(Stop::FrontLost));

    assert!(w.stderr().contains("cannot bind the proxy front again"));
    assert_eq!(run.finish(Stop::FrontLost), 1);
    assert_eq!(w.row().lease_owner, None);
}

#[test]
fn a_supervisor_that_lost_its_lease_leaves_its_successors_socket() {
    let w = World::new([]);
    let deps = w.deps();
    let old = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.clock.sleep(LEASE);
    let new = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let socket = new.socket().to_path_buf();

    assert_eq!(old.tick(), Some(Stop::LostLease));
    assert_eq!(old.finish(Stop::LostLease), 1);

    assert!(ask(bridge(&socket)).starts_with(REFUSAL));
}

#[test]
fn a_supervisor_that_cannot_claim_the_lease_leaves_the_running_socket() {
    let w = World::new([]);
    let deps = w.deps();
    let running = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let socket = running.socket();
    let bound = fs::symlink_metadata(socket).unwrap().ino();

    assert_eq!(start(&deps, w.id).err(), Some(0));

    assert_eq!(fs::symlink_metadata(socket).unwrap().ino(), bound);
    assert!(ask(bridge(socket)).starts_with(REFUSAL));
}

#[test]
fn a_supervisor_that_lost_its_lease_does_not_bind_a_removed_socket_again() {
    let w = World::new([]);
    let deps = w.deps();
    let old = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.clock.sleep(LEASE);
    let new = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let socket = new.socket().to_path_buf();
    let tunnel = bridge(&socket);
    fs::remove_file(&socket).unwrap();

    assert_eq!(old.tick(), Some(Stop::LostLease));

    assert!(ask(tunnel).is_empty());
    assert_eq!(old.finish(Stop::LostLease), 1);
    w.clock.sleep(TICK);
    assert!(new.tick().is_none());
    assert!(ask(tunnel).starts_with(REFUSAL));
}

#[test]
fn lost_lease_stops_the_proxy_from_accepting() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let socket = run.socket().to_path_buf();
    assert!(UnixStream::connect(&socket).is_ok());
    w.clock.sleep(LEASE);
    let taken = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(taken, Lease::Claimed));
    assert!(matches!(run.tick(), Some(Stop::LostLease)));
    assert_eq!(run.finish(Stop::LostLease), 1);

    assert!(UnixStream::connect(&socket).is_err());
}

/// Adds a second working Box to `w`, with a reserved proxy row and a saved spec.
fn second_box(w: &World) -> BoxId {
    let id = working_box(&w.store, &w.driver, "other");
    let nonce = Secret::new(b"other-nonce".to_vec());
    drop(
        w.store
            .reserve(id, &nonce, &FakeRng::default(), LEASE)
            .unwrap(),
    );
    let spec = ProxySpec {
        box_id: id,
        ..read_spec(w.dir.path(), w.id).unwrap()
    };
    write_spec(w.dir.path(), &spec).unwrap();
    id
}

/// Starts the supervisors of `a` and then `b`, stops `b`, and expects `a`'s tunnel to still
/// reach a front.
fn assert_own_sockets(a: (&World, BoxId), b: (&World, BoxId)) {
    let (deps_a, deps_b) = (a.0.deps(), b.0.deps());
    let run_a = start(&deps_a, a.1)
        .map_err(|c| format!("exit {c}"))
        .unwrap();
    let run_b = start(&deps_b, b.1)
        .map_err(|c| format!("exit {c}"))
        .unwrap();
    let (tunnel_a, tunnel_b) = (bridge(run_a.socket()), bridge(run_b.socket()));
    assert!(ask(tunnel_b).starts_with(REFUSAL));

    assert_eq!(run_b.finish(Stop::Signal), 0);

    assert!(ask(tunnel_b).is_empty());
    assert!(ask(tunnel_a).starts_with(REFUSAL));
}

#[test]
fn two_boxes_of_one_state_dir_each_keep_their_own_socket() {
    let w = World::new([]);
    let other = second_box(&w);
    assert_own_sockets((&w, w.id), (&w, other));
}

#[test]
fn one_box_id_in_two_state_dirs_keeps_two_sockets() {
    let a = World::new([]);
    let mut b = World::new([]);
    b.sockets.clone_from(&a.sockets);
    assert_eq!(a.id, b.id);
    assert_own_sockets((&a, a.id), (&b, b.id));
}
