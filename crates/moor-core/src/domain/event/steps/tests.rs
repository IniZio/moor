//! Tests for the lifecycle and session step-to-event tables.

use super::*;

type Ev = Event;

fn s(v: &str) -> String {
    v.to_string()
}

fn ev(from: Option<BoxLifecycle>, to: BoxLifecycle, input: &BoxLifecycleInput) -> Vec<Ev> {
    lifecycle_events(from, to, input)
}

#[test]
fn lifecycle_events_table() {
    use BoxLifecycle as L;
    use BoxLifecycleInput as I;
    assert_eq!(ev(None, L::Checking, &I::Insert), vec![Event::Started]);
    assert_eq!(
        ev(
            Some(L::Checking),
            L::Rejected,
            &I::Rejected { reason: s("no") }
        ),
        vec![Event::Rejected { reason: s("no") }]
    );
    assert_eq!(
        ev(
            Some(L::Checking),
            L::Pending,
            &I::AtCapacity { max_boxes: 3 }
        ),
        vec![Event::Pending { max_boxes: 3 }]
    );
    for (from, input) in [
        (L::Checking, I::Admitted),
        (L::Pending, I::Promoted),
        (L::SetupFailed, I::Retry),
    ] {
        assert_eq!(
            ev(Some(from), L::SettingUp, &input),
            vec![Event::SettingUp { from }]
        );
    }
    assert_eq!(
        ev(None, L::SettingUp, &I::Admitted),
        vec![Event::SettingUp { from: L::SettingUp }]
    );
    assert_eq!(
        ev(
            Some(L::SettingUp),
            L::Ready,
            &I::Ready {
                setup: SetupOutcome::Ok
            }
        ),
        vec![Event::Working]
    );
    assert_eq!(
        ev(
            Some(L::SettingUp),
            L::Ready,
            &I::Ready {
                setup: SetupOutcome::Failed { line: s("x") }
            }
        ),
        vec![Event::Working, Event::SetupFailed { line: s("x") }]
    );
    lifecycle_events_table_rest();
}

fn lifecycle_events_table_rest() {
    use BoxLifecycle as L;
    use BoxLifecycleInput as I;
    assert_eq!(
        ev(
            Some(L::SettingUp),
            L::SetupFailed,
            &I::SetupFailed {
                cause: s("c"),
                which: None
            }
        ),
        vec![Event::SetupFailed { line: s("c") }]
    );
    assert_eq!(
        ev(
            Some(L::SettingUp),
            L::SetupFailed,
            &I::SetupFailed {
                cause: s("c"),
                which: Some(s("gh"))
            }
        ),
        vec![
            Event::CredentialFailed {
                which: s("gh"),
                error: s("c")
            },
            Event::SetupFailed { line: s("c") }
        ]
    );
    for auto in [true, false] {
        assert_eq!(
            ev(Some(L::Ready), L::Asleep, &I::Sleep { auto }),
            vec![Event::Asleep { auto }]
        );
    }
    assert_eq!(ev(Some(L::Asleep), L::Ready, &I::Wake), vec![Event::Woke]);
    assert_eq!(
        ev(Some(L::Ready), L::Closed, &I::Close),
        vec![Event::Closed { from: L::Ready }]
    );
    assert_eq!(
        ev(None, L::Closed, &I::Close),
        vec![Event::Closed { from: L::Closed }]
    );
}

#[test]
fn session_events_table() {
    use SessionInput as I;
    assert_eq!(session_events(&I::Open), Vec::<Ev>::new());
    assert_eq!(session_events(&I::End), Vec::<Ev>::new());
    assert_eq!(session_events(&I::Started), vec![Event::Working]);
    assert_eq!(session_events(&I::AgentQuiet), vec![Event::Idled]);
    assert_eq!(session_events(&I::AgentBusy), vec![Event::Busy]);
    assert_eq!(session_events(&I::RunStarted), vec![Event::Busy]);
}
