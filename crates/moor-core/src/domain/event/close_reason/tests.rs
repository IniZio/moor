use super::CloseReason;

fn every() -> Vec<CloseReason> {
    vec![
        CloseReason::requested("github:ada"),
        CloseReason::SetupFailedExpired,
        CloseReason::RunEnded,
        CloseReason::PrMerged,
        CloseReason::PrClosed,
        CloseReason::Idle,
        CloseReason::CloseFailed,
        CloseReason::WorkflowCancelled,
        CloseReason::StartLimit,
        CloseReason::LateFiring,
        CloseReason::Superseded,
    ]
}

#[test]
fn every_reason_reads_back_from_its_event_and_its_column() {
    for r in every() {
        assert_eq!(CloseReason::from_event(r.code(), r.by()), Some(r.clone()));
        assert_eq!(CloseReason::from_stored(&r.stored()), Some(r));
    }
}

#[test]
fn a_person_close_names_the_person() {
    let r = CloseReason::requested("github:ada");
    assert_eq!(r.code(), "requested");
    assert_eq!(r.by(), Some("github:ada"));
    assert_eq!(r.stored(), "requested:github:ada");
    assert_eq!(r.says(), "by github:ada");
}

#[test]
fn a_person_close_without_a_person_or_an_unknown_code_reads_as_none() {
    assert_eq!(CloseReason::from_event("requested", None), None);
    assert_eq!(CloseReason::from_stored("requested:"), None);
    assert_eq!(CloseReason::from_stored("someday"), None);
    assert_eq!(CloseReason::from_event("someday", None), None);
}

#[test]
fn a_reason_moor_chose_says_moor_closed_it() {
    for r in every().into_iter().filter(|r| r.by().is_none()) {
        assert!(r.says().starts_with("by "), "{r:?}: {}", r.says());
    }
}
