//! Tests for the Box state machine.

use super::{BoxInput, BoxState};
use crate::domain::Illegal;
use crate::name::BoxHandle;

use BoxState::{Asleep, Deleted, Failed, Gone, Ready, ReleaseFailed, Starting};

/// One sample of every input variant.
fn inputs() -> Vec<BoxInput> {
    vec![
        BoxInput::Requested,
        BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        },
        BoxInput::AcquireFailed {
            cause: "c".to_owned(),
            attempted: None,
        },
        BoxInput::Sleep,
        BoxInput::Wake,
        BoxInput::DriverMissing,
        BoxInput::Released,
        BoxInput::ReleaseErr {
            cause: "c".to_owned(),
        },
    ]
}

/// Picks the sample input with the given name.
fn input(name: &str) -> BoxInput {
    inputs()
        .into_iter()
        .find(|i| i.name() == name)
        .expect("known input name")
}

/// Every legal (from, input name, to) triple.
const LEGAL: [(BoxState, &str, BoxState); 15] = [
    (Starting, "acquired", Ready),
    (Starting, "acquire_failed", Failed),
    (Ready, "sleep", Asleep),
    (Asleep, "wake", Ready),
    (Ready, "driver_missing", Gone),
    (Asleep, "driver_missing", Gone),
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
    for from in BoxState::ALL {
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
    assert_eq!(checked, BoxState::ALL.len() * inputs().len() - LEGAL.len());
}

/// `initial` accepts only `Requested`.
#[test]
fn initial_only_requested() {
    assert_eq!(BoxState::initial(&BoxInput::Requested), Ok(Starting));
    for i in inputs().into_iter().filter(|i| i.name() != "requested") {
        assert_eq!(
            BoxState::initial(&i),
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
    for s in BoxState::ALL {
        assert_eq!(BoxState::parse(s.as_str()), Some(s));
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            format!("\"{}\"", s.as_str())
        );
        assert_eq!(
            serde_json::from_str::<BoxState>(&format!("\"{}\"", s.as_str())).unwrap(),
            s
        );
    }
    assert_eq!(BoxState::parse("stopped"), None);
    assert_eq!(BoxState::parse(""), None);
}

/// Only Starting, Ready and Asleep count toward capacity.
#[test]
fn capacity() {
    for s in BoxState::ALL {
        assert_eq!(
            s.counts_toward_capacity(),
            matches!(s, Starting | Ready | Asleep),
            "{s:?}"
        );
    }
}
