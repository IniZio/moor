use moor_core::view::AttachEnd;

use super::{Card, Choice, Ended};
use crate::boxes::{Activity, Phase, Shown};

fn shown(phase: Phase) -> Shown {
    Shown {
        name: "fix".into(),
        repo: None,
        branch: Some("moor/fix".into()),
        phase,
    }
}

fn text(ended: &Ended, now: Option<Phase>) -> String {
    let now = now.map(shown);
    Card::after(ended, now.as_ref(), Some("moor/fix".into()))
        .lines("◈ moor/fix")
        .join("\n")
}

const KEYS: &str = "r attach · q close this pane · ctrl+] detaches while attached";
const LIVE: Option<Phase> = Some(Phase::Live(Activity::Idle));

#[test]
fn a_box_gone_from_the_list_shows_where_its_work_went() {
    assert_eq!(
        text(&Ended::Relay(AttachEnd::Agent), None),
        "◈ moor/fix\n\nclosed · branch moor/fix · PR: gh pr view moor/fix\n\nq close this pane"
    );
}

#[test]
fn an_asleep_box_offers_to_wake_and_attach() {
    assert_eq!(
        text(&Ended::Skipped, Some(Phase::Asleep)),
        "◈ moor/fix\n\nasleep · r wakes it and attaches\n\nr wake and attach · q close this pane"
    );
}

#[test]
fn each_end_of_a_live_relay_has_its_own_line() {
    let line = |ended: Ended| text(&ended, LIVE).lines().nth(2).unwrap().to_owned();
    assert_eq!(
        line(Ended::Relay(AttachEnd::Client)),
        "detached · the agent keeps running"
    );
    assert_eq!(
        line(Ended::Relay(AttachEnd::Agent)),
        "the agent's terminal ended · the Box is still up"
    );
    assert_eq!(
        line(Ended::Relay(AttachEnd::Link)),
        "the link to the Box dropped"
    );
    assert_eq!(line(Ended::Refused("no".into())), "attach refused: no");
    assert!(text(&Ended::Relay(AttachEnd::Client), LIVE).ends_with(KEYS));
}

#[test]
fn r_and_enter_wake_an_asleep_box_and_reattach_a_live_one() {
    for key in *b"rR\r" {
        assert_eq!(Card::Asleep.key(key), Choice::Wake);
        assert_eq!(Card::Detached.key(key), Choice::Attach);
    }
}

#[test]
fn r_on_a_closed_box_says_so_and_offers_nothing() {
    let closed = Card::Closed { branch: None };
    assert_eq!(
        closed.key(b'r'),
        Choice::Note("closed: there is nothing to attach")
    );
    assert_eq!(
        closed.key(b'\r'),
        Choice::Note("closed: there is nothing to attach")
    );
}

#[test]
fn q_closes_the_pane_from_every_card_and_other_keys_do_nothing() {
    for card in [Card::Asleep, Card::Detached, Card::Closed { branch: None }] {
        assert_eq!(card.key(b'q'), Choice::Close);
        assert_eq!(card.key(b'x'), Choice::Stay);
    }
}
