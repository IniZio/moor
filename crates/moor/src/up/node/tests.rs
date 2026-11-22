#![allow(clippy::unwrap_used, reason = "test code")]

use std::net::{Ipv4Addr, TcpListener};

use super::{Ports, PORTS_FILE};

#[test]
fn a_state_dir_with_no_kept_ports_picks_three_free_ones_past_a_taken_port_and_keeps_them() {
    let first = tempfile::tempdir().unwrap();
    let base = Ports::read_or_pick(first.path()).unwrap();
    std::fs::remove_file(first.path().join(PORTS_FILE)).unwrap();
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, base.agents)).unwrap();
    let picked = Ports::read_or_pick(first.path()).unwrap();
    drop(held);
    assert_ne!(picked.agents, base.agents, "{picked:?} after {base:?}");
    assert!(
        picked.agents != picked.listen && picked.listen != picked.egress,
        "{picked:?}"
    );
    assert!(first.path().join(PORTS_FILE).exists());
    assert_eq!(Ports::read_or_pick(first.path()).unwrap(), picked);
}
