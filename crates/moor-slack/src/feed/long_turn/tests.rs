use serde_json::json;

use super::*;

const EVERY: i64 = 20 * 60;

fn working_since(secs: i64) -> View {
    let mut v = View::default();
    v.fold("working", &json!({}), secs * 1000);
    v
}

#[test]
fn a_threshold_is_reached_after_each_whole_interval_of_the_same_turn() {
    let v = working_since(100);
    assert_eq!(reached(&v, 100 + EVERY - 1, EVERY), None);
    assert_eq!(reached(&v, 100 + EVERY, EVERY), Some(1));
    assert_eq!(reached(&v, 100 + 2 * EVERY - 1, EVERY), Some(1));
    assert_eq!(reached(&v, 100 + 2 * EVERY, EVERY), Some(2));
}

#[test]
fn no_threshold_is_reached_once_the_turn_ended_or_before_it_began() {
    let mut v = working_since(0);
    v.fold("idled", &json!({}), 100_000);
    assert_eq!(reached(&v, 10 * EVERY, EVERY), None);
    assert_eq!(reached(&View::default(), 10 * EVERY, EVERY), None);
}

#[test]
fn the_line_names_the_box_the_time_and_what_it_is_doing_when_known() {
    let name = BoxName::parse_allow_bare("slack-1-1").unwrap();
    assert_eq!(
        line(&name, Some("Bash: cargo test"), 1, EVERY),
        "`slack-1-1` has been working for 20m. Now: `Bash: cargo test`. \
         Reply here to redirect it, or ask me to stop it."
    );
    assert_eq!(
        line(&name, None, 3, EVERY),
        "`slack-1-1` has been working for 1h 00m. \
         Reply here to redirect it, or ask me to stop it."
    );
}

#[test]
fn each_threshold_of_each_turn_has_its_own_key() {
    let keys = [key(5, 1), key(5, 2), key(6, 1), key(6, 2)];
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len(), "{keys:?}");
}
