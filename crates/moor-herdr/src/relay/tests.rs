use std::sync::mpsc;
use std::time::Duration;

use moor_client::ClientError;

use super::{closed_while, ended, Event};
use crate::card::Ended;

const TICK: Duration = Duration::from_millis(5);

#[test]
fn waiting_polls_until_the_box_is_no_longer_starting() {
    let (_tx, rx) = mpsc::channel();
    let mut polls = 0;
    let closed = closed_while(&rx, TICK, || {
        polls += 1;
        polls < 3
    });
    assert!(!closed);
    assert_eq!(polls, 3);
}

#[test]
fn q_closes_the_pane_while_waiting_and_other_keys_do_not() {
    let (tx, rx) = mpsc::channel();
    tx.send(Event::Keys(b"x".to_vec())).unwrap();
    tx.send(Event::Resized(80, 24)).unwrap();
    tx.send(Event::Keys(b"q".to_vec())).unwrap();
    assert!(closed_while(&rx, TICK, || true));
}

#[test]
fn a_client_error_ends_the_relay_with_its_message() {
    let err = ClientError::from(std::io::Error::other("socket gone"));
    assert!(matches!(ended(Err(err)), Ended::Refused(why) if why.contains("socket gone")));
}
