//! A menu choice that names the prompt it answers lands only on that prompt.

use moor_core::progress::ask_id;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::Steer;
use moor_core::{ErrorKind, SteerReq};

use super::{fx_with, menu, steered, working};
use crate::ops::steer::run;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// A choice of option `n` that answers the prompt named `ask`.
fn answer(n: u32, ask: &str) -> SteerReq {
    SteerReq {
        ask: Some(ask.to_owned()),
        ..menu(n)
    }
}

fn options() -> Vec<String> {
    vec!["Apple".to_owned(), "Banana".to_owned()]
}

#[test]
fn a_choice_for_the_prompt_the_agent_waits_at_is_sent() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    let ask = ask_id(Some("Which fruit?"), None, &options());
    run(&f.cx, &answer(2, &ask)).unwrap();
    assert_eq!(f.engine.steers(), [("agent-1".to_owned(), Steer::Menu(2))]);
}

#[test]
fn a_choice_for_an_earlier_prompt_is_refused_and_sends_nothing() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    let earlier = ask_id(Some("Allow npm test?"), None, &options());
    let err = run(&f.cx, &answer(1, &earlier)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("asks something else"),
        "{err:?}"
    );
    assert!(f.engine.steers().is_empty());
}

#[test]
fn a_choice_for_a_prompt_the_agent_left_is_refused_and_sends_nothing() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let err = run(&f.cx, &answer(1, "gone")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn a_choice_naming_no_prompt_still_answers_whatever_is_asked() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    run(&f.cx, &menu(1)).unwrap();
    assert_eq!(f.engine.steers(), [("agent-1".to_owned(), Steer::Menu(1))]);
}

#[test]
fn the_you_row_of_a_choice_is_the_option_text_not_its_number() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    run(&f.cx, &menu(2)).unwrap();
    assert_eq!(f.engine.steers(), [("agent-1".to_owned(), Steer::Menu(2))]);
    let said: Vec<_> = steered(&f).iter().map(|p| p["message"].clone()).collect();
    assert_eq!(said, [serde_json::json!("Banana")]);
}

#[test]
fn the_you_row_of_a_typed_answer_is_what_the_person_wrote() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    let req = SteerReq {
        text: Some("kiwi".to_owned()),
        ..menu(3)
    };
    run(&f.cx, &req).unwrap();
    let said: Vec<_> = steered(&f).iter().map(|p| p["message"].clone()).collect();
    assert_eq!(said, [serde_json::json!("kiwi")]);
}

#[test]
fn a_choice_with_text_picks_the_entry_then_types_the_text() {
    let asking = agent::asking("Which fruit?", &["Apple", "Banana"], Ts::from_millis(0));
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    let ask = ask_id(Some("Which fruit?"), None, &options());
    let req = SteerReq {
        text: Some("kiwi".to_owned()),
        ..answer(3, &ask)
    };
    run(&f.cx, &req).unwrap();
    assert_eq!(
        f.engine.steers(),
        [
            ("agent-1".to_owned(), Steer::Menu(3)),
            ("agent-1".to_owned(), Steer::Text("kiwi".to_owned())),
        ]
    );
}
