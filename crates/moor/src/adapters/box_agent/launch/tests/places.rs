//! The agent's files and Claude live in the agent place.

use super::*;

#[test]
fn a_restore_puts_the_archive_and_unpacks_it_in_the_agent_place() {
    let d = driver(FakeDriver::new());
    agent(&d).restore_session(&req(), "s1", b"archive").unwrap();
    assert_eq!(d.places("put_file"), [Place::Agent]);
    assert_eq!(d.places("exec"), [Place::Agent]);
}

#[test]
fn a_stop_ends_the_terminal_in_the_agent_place() {
    let d = driver(FakeDriver::new());
    let kit = agent(&d);
    let sref = kit.launch(&req()).unwrap();
    let before = d.places("exec").len();
    kit.stop(&sref).unwrap();
    assert_eq!(d.places("exec").split_at(before).1, [Place::Agent]);
}
