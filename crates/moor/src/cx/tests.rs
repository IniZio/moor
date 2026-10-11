//! Tests for `Cx`.

use super::Cx;
use crate::adapters::clock::FakeClock;
use moor_core::seam::clock::Ts;
use moor_core::Warning;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Context over a fake clock starting at 1000 ms.
fn cx(state_dir: Option<PathBuf>) -> Cx {
    Cx::new(state_dir, Arc::new(FakeClock::new(Ts::from_millis(1000))))
}

/// Build a warning.
fn w(code: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message: format!("msg {code}"),
    }
}

#[test]
fn warnings_drain_in_order() {
    let cx = cx(None);
    cx.warn(w("a"));
    cx.warn(w("b"));
    let got = cx.take_warnings();
    assert_eq!(got.len(), 2);
    assert_eq!(got.first().map(|x| x.code.as_str()), Some("a"));
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn state_dir_round_trips() {
    assert_eq!(cx(None).state_dir(), None);
    let cx = cx(Some(PathBuf::from("/s")));
    assert_eq!(cx.state_dir(), Some(Path::new("/s")));
}

#[test]
fn clock_is_the_injected_one() {
    let cx = cx(None);
    assert_eq!(cx.clock().now(), Ts::from_millis(1000));
    cx.clock().sleep(Duration::from_millis(250));
    assert_eq!(cx.clock().now(), Ts::from_millis(1250));
}
