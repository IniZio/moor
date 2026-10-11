//! Tests for `Cx`.

use super::Cx;
use moor_core::Warning;
use std::path::{Path, PathBuf};

/// Build a warning.
fn w(code: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message: format!("msg {code}"),
    }
}

#[test]
fn warnings_drain_in_order() {
    let cx = Cx::new(None);
    cx.warn(w("a"));
    cx.warn(w("b"));
    let got = cx.take_warnings();
    assert_eq!(got.len(), 2);
    assert_eq!(got.first().map(|x| x.code.as_str()), Some("a"));
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn state_dir_round_trips() {
    assert_eq!(Cx::new(None).state_dir(), None);
    let cx = Cx::new(Some(PathBuf::from("/s")));
    assert_eq!(cx.state_dir(), Some(Path::new("/s")));
}
