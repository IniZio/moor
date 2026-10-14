//! Tests for the pane section of `show`.

use std::sync::Arc;

use moor_core::activity::{PaneRead, PaneStatus};
use moor_core::{Secret, ShowReq};

use super::super::run;
use super::{engine_cx, pane_only, req, working};
use crate::ops::common::proxy::LEASE;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// Show task `a` over a pane of fifty numbered lines.
fn show_pane(pane: bool, session: Option<&str>) -> moor_core::view::ShowView {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", session);
    let mut probes = pane_only(PaneStatus::Working);
    if let PaneRead::Fresh(obs) = &mut probes.pane {
        obs.tail = (1..=50).fold(String::new(), |t, n| t + &format!("line {n}\n"));
    }
    let engine = Arc::new(FakeEngine::new().probes("s-a", probes));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let r = ShowReq {
        pane,
        ..req(Some("a"), None)
    };
    let v = run(&cx, &r).unwrap();
    assert_eq!(engine.probe_count("s-a"), usize::from(session.is_some()));
    v
}

#[test]
fn pane_flag_returns_the_last_lines_and_status() {
    let v = show_pane(true, Some("s-a"));
    let pane = v.pane.unwrap();
    assert_eq!(pane.status, "live");
    assert_eq!(pane.tail.lines().count(), 40);
    assert_eq!(pane.tail.lines().next(), Some("line 11"));
    assert!(v.reports.is_none());
}

#[test]
fn pane_is_null_without_the_flag_or_a_session() {
    assert!(show_pane(false, Some("s-a")).pane.is_none());
    assert!(show_pane(true, None).pane.is_none());
}

const NONCE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde";

/// Show task `a` over a pane whose last line is a push through the proxy.
fn show_push(nonce: &str, proxy_row: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    if proxy_row {
        let nonce = Secret::new(nonce.as_bytes().to_vec());
        let reserved = cx.store().unwrap().reserve(id, &nonce, cx.rng(), LEASE);
        assert!(reserved.is_ok());
    }
    let mut probes = pane_only(PaneStatus::Working);
    if let PaneRead::Fresh(obs) = &mut probes.pane {
        obs.tail = format!("To http://127.0.0.1:21717/t/{nonce}/github/o/n.git\n");
    }
    let engine = Arc::new(FakeEngine::new().probes("s-a", probes));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let r = ShowReq {
        pane: true,
        ..req(Some("a"), None)
    };
    run(&cx, &r).unwrap().pane.unwrap().tail
}

#[test]
fn pane_redacts_the_stored_nonce() {
    let tail = show_push("short-nonce", true);
    assert!(tail.contains("/t/<nonce>/github/o/n.git"), "{tail}");
    assert!(!tail.contains("short-nonce"));
}

#[test]
fn pane_redacts_a_nonce_shaped_segment_without_a_proxy_row() {
    let tail = show_push(NONCE, false);
    assert!(tail.contains("/t/<nonce>/github/o/n.git"), "{tail}");
    assert!(!tail.contains(NONCE));
}
