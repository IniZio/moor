//! A fixed table of `apply` results, readable without the old combined state.
#![allow(clippy::panic, reason = "test assertions")]

use crate::domain::box_state::{
    BoxLifecycle as B, BoxLifecycleInput as L, SetupOutcome, StepError,
};
use crate::domain::box_steps::apply;
use crate::domain::event::{lifecycle_events, session_events, Event};
use crate::domain::session_state::{SessionInput as S, SessionState as P};

/// What a row expects.
#[derive(Debug)]
enum Want {
    Ok(B, P),
    Illegal,
    Blocked(usize),
}

struct Row {
    lifecycle: B,
    primary: P,
    others: Vec<P>,
    l: Option<L>,
    s: Option<S>,
    want: Want,
}

fn row(lifecycle: B, primary: P, l: Option<L>, s: Option<S>, want: Want) -> Row {
    Row {
        lifecycle,
        primary,
        others: vec![],
        l,
        s,
        want,
    }
}

fn ready(setup: SetupOutcome) -> L {
    L::Ready { setup }
}

fn failed(cause: &str, which: Option<&str>) -> L {
    L::SetupFailed {
        cause: cause.into(),
        which: which.map(Into::into),
    }
}

fn rejected() -> L {
    L::Rejected { reason: "r".into() }
}

#[allow(clippy::too_many_lines, reason = "one row per transition")]
fn table() -> Vec<Row> {
    let at_cap = || L::AtCapacity { max_boxes: 1 };
    let sleep = |auto| Some(L::Sleep { auto });
    let mut rows = vec![
        // Box-only steps.
        row(
            B::Checking,
            P::Starting,
            Some(rejected()),
            None,
            Want::Ok(B::Rejected, P::Starting),
        ),
        row(
            B::Checking,
            P::Starting,
            Some(at_cap()),
            None,
            Want::Ok(B::Pending, P::Starting),
        ),
        row(
            B::Checking,
            P::Starting,
            Some(L::Admitted),
            None,
            Want::Ok(B::SettingUp, P::Starting),
        ),
        row(
            B::Pending,
            P::Starting,
            Some(L::Promoted),
            None,
            Want::Ok(B::SettingUp, P::Starting),
        ),
        row(
            B::SettingUp,
            P::Starting,
            Some(failed("c", None)),
            None,
            Want::Ok(B::SetupFailed, P::Starting),
        ),
        row(
            B::SettingUp,
            P::Starting,
            Some(failed("e", Some("w"))),
            None,
            Want::Ok(B::SetupFailed, P::Starting),
        ),
        row(
            B::SetupFailed,
            P::Starting,
            Some(L::Retry),
            None,
            Want::Ok(B::SettingUp, P::Starting),
        ),
        row(
            B::Rejected,
            P::Starting,
            Some(L::Admitted),
            None,
            Want::Illegal,
        ),
        row(B::Ready, P::Working, Some(L::Admitted), None, Want::Illegal),
        row(B::Closed, P::Ended, Some(L::Wake), None, Want::Illegal),
        // Ready starts the primary Session.
        row(
            B::SettingUp,
            P::Starting,
            Some(ready(SetupOutcome::Ok)),
            Some(S::Started),
            Want::Ok(B::Ready, P::Working),
        ),
        row(
            B::SettingUp,
            P::Starting,
            Some(ready(SetupOutcome::Failed { line: "l".into() })),
            Some(S::Started),
            Want::Ok(B::Ready, P::Working),
        ),
        row(
            B::Checking,
            P::Starting,
            Some(ready(SetupOutcome::Ok)),
            Some(S::Started),
            Want::Illegal,
        ),
        // Session-only steps need a Ready Box.
        row(
            B::Ready,
            P::Working,
            None,
            Some(S::AgentQuiet),
            Want::Ok(B::Ready, P::Idle),
        ),
        row(
            B::Ready,
            P::Idle,
            None,
            Some(S::AgentBusy),
            Want::Ok(B::Ready, P::Working),
        ),
        row(
            B::Ready,
            P::Idle,
            None,
            Some(S::RunStarted),
            Want::Ok(B::Ready, P::Working),
        ),
        row(
            B::SettingUp,
            P::Starting,
            None,
            Some(S::AgentQuiet),
            Want::Illegal,
        ),
        row(B::Asleep, P::Idle, None, Some(S::AgentBusy), Want::Illegal),
        row(
            B::Checking,
            P::Starting,
            None,
            Some(S::RunStarted),
            Want::Illegal,
        ),
        // An ended primary Session skips Session inputs.
        row(
            B::Ready,
            P::Ended,
            None,
            Some(S::AgentQuiet),
            Want::Ok(B::Ready, P::Ended),
        ),
        row(
            B::Ready,
            P::Ended,
            None,
            Some(S::AgentBusy),
            Want::Ok(B::Ready, P::Ended),
        ),
        row(
            B::Ready,
            P::Ended,
            None,
            Some(S::RunStarted),
            Want::Ok(B::Ready, P::Ended),
        ),
        // Sleep.
        row(
            B::Ready,
            P::Idle,
            sleep(true),
            None,
            Want::Ok(B::Asleep, P::Idle),
        ),
        row(B::Ready, P::Working, sleep(true), None, Want::Illegal),
        row(
            B::Ready,
            P::Ended,
            sleep(true),
            None,
            Want::Ok(B::Asleep, P::Ended),
        ),
        row(
            B::Ready,
            P::Working,
            sleep(false),
            None,
            Want::Ok(B::Asleep, P::Idle),
        ),
        row(
            B::Ready,
            P::Idle,
            sleep(false),
            None,
            Want::Ok(B::Asleep, P::Idle),
        ),
        row(B::SettingUp, P::Starting, sleep(false), None, Want::Illegal),
        // Wake and close.
        row(
            B::Asleep,
            P::Idle,
            Some(L::Wake),
            Some(S::AgentBusy),
            Want::Ok(B::Ready, P::Working),
        ),
        row(
            B::Asleep,
            P::Ended,
            Some(L::Wake),
            Some(S::AgentBusy),
            Want::Ok(B::Ready, P::Ended),
        ),
        row(
            B::Ready,
            P::Idle,
            Some(L::Wake),
            Some(S::AgentBusy),
            Want::Illegal,
        ),
        row(
            B::Ready,
            P::Working,
            Some(L::Close),
            Some(S::End),
            Want::Ok(B::Closed, P::Ended),
        ),
        row(
            B::Ready,
            P::Ended,
            Some(L::Close),
            Some(S::End),
            Want::Ok(B::Closed, P::Ended),
        ),
        row(
            B::Asleep,
            P::Idle,
            Some(L::Close),
            Some(S::End),
            Want::Ok(B::Closed, P::Ended),
        ),
        row(
            B::Closed,
            P::Ended,
            Some(L::Close),
            Some(S::End),
            Want::Illegal,
        ),
    ];
    // No input leaves every state as it was.
    for (lc, p) in [
        (B::Checking, P::Starting),
        (B::Ready, P::Working),
        (B::Asleep, P::Idle),
        (B::Closed, P::Ended),
    ] {
        rows.push(row(lc, p, None, None, Want::Ok(lc, p)));
    }
    // Other Sessions gate sleep.
    let with_others = |others: Vec<P>, auto, want| Row {
        others,
        ..row(B::Ready, P::Idle, sleep(auto), None, want)
    };
    rows.push(with_others(vec![P::Working], true, Want::Blocked(1)));
    rows.push(with_others(vec![P::Working], false, Want::Blocked(1)));
    rows.push(with_others(
        vec![P::Working, P::Starting],
        true,
        Want::Blocked(2),
    ));
    rows.push(with_others(
        vec![P::Idle, P::Ended],
        true,
        Want::Ok(B::Asleep, P::Idle),
    ));
    rows
}

#[test]
fn golden_table_of_apply_results() {
    let rows = table();
    for r in &rows {
        let got = apply(
            r.lifecycle,
            r.primary,
            &r.others,
            r.l.as_ref(),
            r.s.as_ref(),
        );
        let ctx = format!(
            "{:?}/{:?} + {:?} + {:?} + {:?}",
            r.lifecycle, r.primary, r.others, r.l, r.s
        );
        match (&r.want, got) {
            (Want::Ok(lc, p), Ok(g)) => assert_eq!(g, (*lc, *p), "{ctx}"),
            (Want::Illegal, Err(StepError::Illegal(_))) => {}
            (Want::Blocked(n), Err(StepError::SleepBlocked { busy })) => {
                assert_eq!(busy, *n, "{ctx}");
            }
            (want, got) => panic!("{ctx}: want {want:?}, got {got:?}"),
        }
    }
    assert_eq!(rows.len(), 43);
}

#[test]
fn golden_setup_failure_with_credential_reports_both_events() {
    let l = failed("e", Some("w"));
    let got = lifecycle_events(Some(B::SettingUp), B::SetupFailed, &l);
    assert_eq!(
        got,
        vec![
            Event::CredentialFailed {
                which: "w".into(),
                error: "e".into(),
            },
            Event::SetupFailed { line: "e".into() },
        ]
    );
}

#[test]
fn golden_setup_failure_without_credential_reports_one_event() {
    let l = failed("c", None);
    let got = lifecycle_events(Some(B::SettingUp), B::SetupFailed, &l);
    assert_eq!(got, vec![Event::SetupFailed { line: "c".into() }]);
}

#[test]
fn golden_box_events_name_the_state_left() {
    assert_eq!(
        lifecycle_events(None, B::Checking, &L::Insert),
        vec![Event::Started]
    );
    assert_eq!(
        lifecycle_events(Some(B::Checking), B::SettingUp, &L::Admitted),
        vec![Event::SettingUp { from: B::Checking }]
    );
    assert_eq!(
        lifecycle_events(Some(B::Ready), B::Closed, &L::Close),
        vec![Event::Closed { from: B::Ready }]
    );
    assert_eq!(
        lifecycle_events(Some(B::Ready), B::Asleep, &L::Sleep { auto: true }),
        vec![Event::Asleep { auto: true }]
    );
    assert_eq!(
        lifecycle_events(Some(B::Ready), B::Asleep, &L::Sleep { auto: false }),
        vec![Event::Asleep { auto: false }]
    );
    assert_eq!(
        lifecycle_events(Some(B::Asleep), B::Ready, &L::Wake),
        vec![Event::Woke]
    );
}

#[test]
fn golden_ready_events_report_a_failed_setup_line() {
    let ok = lifecycle_events(Some(B::SettingUp), B::Ready, &ready(SetupOutcome::Ok));
    assert_eq!(ok, vec![Event::Working]);
    let bad = lifecycle_events(
        Some(B::SettingUp),
        B::Ready,
        &ready(SetupOutcome::Failed { line: "l".into() }),
    );
    assert_eq!(
        bad,
        vec![Event::Working, Event::SetupFailed { line: "l".into() }]
    );
}

#[test]
fn golden_session_events_follow_session_inputs() {
    assert_eq!(session_events(&S::AgentQuiet), vec![Event::Idled]);
    assert_eq!(session_events(&S::AgentBusy), vec![Event::Busy]);
    assert_eq!(session_events(&S::RunStarted), vec![Event::Busy]);
}
