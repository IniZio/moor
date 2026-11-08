#![allow(clippy::unwrap_used, reason = "tests")]

use super::*;

fn ask() -> Ask {
    serde_json::from_value(json!({"pubkey": "k", "name": "hv", "slots": 1})).unwrap()
}

#[test]
fn a_code_past_its_ten_minutes_can_be_neither_confirmed_nor_collected() {
    let pairings = Pairings::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (code, device) = pairings.start(ask(), t0).unwrap();
    let late = t0 + TTL;
    assert!(pairings.waiting(&code, late).is_none());
    assert!(pairings.confirm(&code, late, |_| Ok(json!({}))).is_none());
    assert_eq!(pairings.poll(&device, late), Polled::Expired);
}

#[test]
fn an_answer_confirmed_in_time_but_collected_late_is_gone() {
    let pairings = Pairings::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (code, device) = pairings.start(ask(), t0).unwrap();
    assert!(pairings
        .confirm(&code, t0, |_| Ok(json!({"machine_id": 1})))
        .is_some());
    assert_eq!(pairings.poll(&device, t0 + TTL), Polled::Expired);
}

#[test]
fn a_failed_enroll_leaves_the_code_waiting() {
    let pairings = Pairings::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (code, device) = pairings.start(ask(), t0).unwrap();
    let failed = pairings.confirm(&code, t0, |_| Err("down".to_owned()));
    assert!(matches!(failed, Some(Err(_))));
    assert!(pairings.waiting(&code, t0).is_some());
    assert_eq!(pairings.poll(&device, t0), Polled::Pending);
}

#[test]
fn expired_codes_make_room_for_new_ones() {
    let pairings = Pairings::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for _ in 0..MOST_WAITING {
        assert!(pairings.start(ask(), t0).is_some());
    }
    assert!(pairings.start(ask(), t0).is_none());
    assert!(pairings.start(ask(), t0 + TTL).is_some());
}
