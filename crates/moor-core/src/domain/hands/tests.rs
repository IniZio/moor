//! Tests for the Box state machine.

use super::{HandsInput, HandsState};
use crate::domain::Illegal;
use crate::name::BoxHandle;

use HandsState::{Asleep, Deleted, Failed, Gone, Ready, ReleaseFailed, Starting};

/// One sample of every input variant.
fn inputs() -> Vec<HandsInput> {
    vec![
        HandsInput::Requested,
        HandsInput::Acquired {
            handle: BoxHandle::new("h"),
        },
        HandsInput::AcquireFailed {
            cause: "c".to_owned(),
            attempted: None,
        },
        HandsInput::Sleep,
        HandsInput::Wake,
        HandsInput::DriverMissing,
        HandsInput::Released,
        HandsInput::ReleaseErr {
            cause: "c".to_owned(),
        },
    ]
}

/// Picks the sample input with the given name.
fn input(name: &str) -> HandsInput {
    inputs()
        .into_iter()
        .find(|i| i.name() == name)
        .expect("known input name")
}

/// Every legal (from, input name, to) triple.
const LEGAL: [(HandsState, &str, HandsState); 19] = [
    (Starting, "acquired", Ready),
    (Starting, "acquire_failed", Failed),
    (Failed, "requested", Starting),
    (Gone, "requested", Starting),
    (Ready, "sleep", Asleep),
    (Asleep, "wake", Ready),
    (Ready, "driver_missing", Gone),
    (Asleep, "driver_missing", Gone),
    (Failed, "driver_missing", Gone),
    (Starting, "released", Deleted),
    (Ready, "released", Deleted),
    (Asleep, "released", Deleted),
    (Failed, "released", Deleted),
    (Gone, "released", Deleted),
    (ReleaseFailed, "released", Deleted),
    (Ready, "release_err", ReleaseFailed),
    (Asleep, "release_err", ReleaseFailed),
    (Failed, "release_err", ReleaseFailed),
    (Gone, "release_err", ReleaseFailed),
];

/// Every legal transition yields its target.
#[test]
fn legal_transitions() {
    for (from, name, to) in LEGAL {
        assert_eq!(from.step(&input(name)), Ok(to), "{from:?} {name}");
    }
}

/// Every pair outside the table is `Illegal` with the right names.
#[test]
fn every_other_pair_is_illegal() {
    let mut checked = 0;
    for from in HandsState::ALL {
        for i in inputs() {
            if LEGAL.iter().any(|(f, n, _)| *f == from && *n == i.name()) {
                continue;
            }
            assert_eq!(
                from.step(&i),
                Err(Illegal {
                    from: from.as_str().to_owned(),
                    input: i.name().to_owned(),
                })
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        HandsState::ALL.len() * inputs().len() - LEGAL.len()
    );
}

/// `initial` accepts only `Requested`.
#[test]
fn initial_only_requested() {
    assert_eq!(HandsState::initial(&HandsInput::Requested), Ok(Starting));
    for i in inputs().into_iter().filter(|i| i.name() != "requested") {
        assert_eq!(
            HandsState::initial(&i),
            Err(Illegal {
                from: "none".to_owned(),
                input: i.name().to_owned(),
            })
        );
    }
}

/// `as_str` and `parse` round-trip and match serde.
#[test]
fn as_str_parse_serde() {
    for s in HandsState::ALL {
        assert_eq!(HandsState::parse(s.as_str()), Some(s));
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            format!("\"{}\"", s.as_str())
        );
        assert_eq!(
            serde_json::from_str::<HandsState>(&format!("\"{}\"", s.as_str())).unwrap(),
            s
        );
    }
    assert_eq!(HandsState::parse("stopped"), None);
    assert_eq!(HandsState::parse(""), None);
}

/// Only Starting, Ready and Asleep count toward capacity.
#[test]
fn capacity() {
    for s in HandsState::ALL {
        assert_eq!(
            s.counts_toward_capacity(),
            matches!(s, Starting | Ready | Asleep),
            "{s:?}"
        );
    }
}

/// Position of a state in `HandsState::ALL`; the exhaustive match fails to compile on a new variant.
const fn state_index(s: HandsState) -> usize {
    match s {
        Starting => 0,
        Ready => 1,
        Failed => 2,
        Asleep => 3,
        Gone => 4,
        ReleaseFailed => 5,
        Deleted => 6,
    }
}

/// Position of an input in `inputs()`; the exhaustive match fails to compile on a new variant.
const fn input_index(i: &HandsInput) -> usize {
    match i {
        HandsInput::Requested => 0,
        HandsInput::Acquired { .. } => 1,
        HandsInput::AcquireFailed { .. } => 2,
        HandsInput::Sleep => 3,
        HandsInput::Wake => 4,
        HandsInput::DriverMissing => 5,
        HandsInput::Released => 6,
        HandsInput::ReleaseErr { .. } => 7,
    }
}

/// `ALL` lists every state once, in match order.
#[test]
fn all_states_listed_once_in_order() {
    assert_eq!(HandsState::ALL.len(), 7);
    for (pos, s) in HandsState::ALL.into_iter().enumerate() {
        assert_eq!(state_index(s), pos, "{s:?}");
    }
}

/// The hand-written name equals the serde name and parses back.
#[test]
fn state_names_match_serde_and_parse() {
    for s in HandsState::ALL {
        let json = serde_json::to_value(s).unwrap();
        assert_eq!(json.as_str(), Some(s.as_str()), "{s:?}");
        assert_eq!(HandsState::parse(s.as_str()), Some(s), "{s:?}");
    }
}

/// The sample inputs cover every variant once, with unique names.
#[test]
fn input_samples_cover_every_variant_with_unique_names() {
    let all = inputs();
    assert_eq!(all.len(), 8);
    for (pos, i) in all.iter().enumerate() {
        assert_eq!(input_index(i), pos, "{i:?}");
    }
    let mut names: Vec<_> = all.iter().map(HandsInput::name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
    for n in names {
        assert_eq!(input(n).name(), n);
    }
}
