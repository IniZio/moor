//! Parsing a pull request and a head, and the cause a stored row stands for.
#![allow(clippy::unwrap_used, reason = "test assertions")]

use super::{CommitSha, PullRef, PullTarget, RunCause};
use crate::name::ForgeRepo;
use crate::seam::clock::Ts;
use crate::workflow::{box_name, RunTrigger, WorkflowId};

const HEAD: &str = "0123456789ABCDEF0123456789abcdef01234567";

fn pull() -> PullRef {
    PullTarget::parse("IniZio/moor-pilot-example#65")
        .unwrap()
        .at(CommitSha::parse(HEAD).unwrap())
}

#[test]
fn a_pull_request_is_owner_name_and_number() {
    let t = PullTarget::parse("IniZio/moor-pilot-example#65").unwrap();
    assert_eq!(t.repo, ForgeRepo::new("IniZio", "moor-pilot-example"));
    assert_eq!(t.number, 65);
    assert_eq!(t.to_string(), "IniZio/moor-pilot-example#65");
    for bad in [
        "o/n", "o/n#", "o/n#0", "o/n#+1", "o/n#x", "n#1", "o/n#1#2", "#1",
    ] {
        assert!(PullTarget::parse(bad).is_err(), "{bad}");
    }
    let wire: PullTarget = serde_json::from_str("\"o/n#7\"").unwrap();
    assert_eq!(serde_json::to_string(&wire).unwrap(), "\"o/n#7\"");
}

#[test]
fn a_head_is_forty_hex_characters_kept_lowercase() {
    let sha = CommitSha::parse(HEAD).unwrap();
    assert_eq!(sha.as_str(), HEAD.to_ascii_lowercase());
    assert_eq!(sha.short(), "0123456");
    for bad in [
        "abc1234",
        HEAD.get(1..).unwrap(),
        &format!("{HEAD}0"),
        &HEAD.replace('0', "g"),
    ] {
        assert!(CommitSha::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_pull_request_gives_its_box_four_variables() {
    let env = pull().env().map(|(k, v)| format!("{k}={v}"));
    assert_eq!(
        env,
        [
            "MOOR_PULL_REPO=IniZio/moor-pilot-example".to_owned(),
            "MOOR_PULL_NUMBER=65".to_owned(),
            format!("MOOR_PULL_HEAD_SHA={}", HEAD.to_ascii_lowercase()),
            "MOOR_PULL_URL=https://github.com/IniZio/moor-pilot-example/pull/65".to_owned(),
        ]
    );
}

#[test]
fn only_a_trigger_and_subject_that_fit_make_a_cause() {
    let p = pull();
    let cause = |t, p| RunCause::from_parts(t, p);
    assert_eq!(cause(RunTrigger::Cron, None), Some(RunCause::Cron));
    assert_eq!(cause(RunTrigger::Cron, Some(p.clone())), None);
    assert_eq!(cause(RunTrigger::PullRequest, None), None);
    let event = cause(RunTrigger::PullRequest, Some(p.clone())).unwrap();
    assert_eq!(event.subject(), Some(&p));
    assert_eq!(event.trigger(), RunTrigger::PullRequest);
    let by_hand = cause(RunTrigger::Manual, Some(p.clone())).unwrap();
    assert_eq!(by_hand.subject(), Some(&p));
    assert_eq!(cause(RunTrigger::Manual, None).unwrap().subject(), None);
}

#[test]
fn a_box_for_a_pull_request_names_it_and_a_manual_one_keeps_its_stamp() {
    let at = Ts::from_millis(5_000);
    let name = |cause| box_name(WorkflowId::new(3), "review", at, &cause).unwrap();
    let event = name(RunCause::PullRequest { pull: pull() });
    assert_eq!(event.as_str(), "review/w3-pr65-0123456");
    let by_hand = name(RunCause::Manual { pull: Some(pull()) });
    assert_eq!(by_hand.as_str(), "review/w3-pr65-0123456-m19700101000005");
}
