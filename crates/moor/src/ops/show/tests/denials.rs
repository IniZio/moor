//! Tests for the sandbox denials section of `show`.

use std::sync::Arc;

use moor_core::boxscript::denials::DENIAL_LINE_CHARS;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::Secret;

use super::super::run;
use super::{add, cx, ready_box, req};
use crate::testing::driver::FakeDriver;

/// Boxes whose workdir lies inside the macOS layout of task `a`.
const MAC_ROOT: &str = "/Users/Shared/moor-n/a/work";

/// Show task `a` over `driver` after giving it a Ready Box; returns the view and the driver.
fn show(driver: FakeDriver) -> (moor_core::view::ShowView, Arc<FakeDriver>) {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    (run(&cx, &req(Some("a"), None)).unwrap(), driver)
}

#[test]
fn mac_task_lists_its_denials() {
    let report = "moor-denials: report\nmoor-denials: log\n\
        2026-01-01 00:00:00.0 Sandbox: git(42) deny(1) file-write-create /Users/Shared/moor-n/a/home/x\n";
    let (v, driver) = show(
        FakeDriver::new()
            .work_root(MAC_ROOT)
            .exec_stdout(["", report]),
    );
    let execs = driver.execs();
    assert_eq!(execs.len(), 2);
    let spec = &execs.last().unwrap().1;
    assert_eq!(spec.argv.first().map(String::as_str), Some("sh"));
    assert_eq!(spec.argv.get(1).map(String::as_str), Some("-c"));
    assert!(spec.argv.get(2).unwrap().contains("/Users/Shared/moor-n/a"));
    let got = v.sandbox_denials.unwrap();
    assert_eq!(
        got,
        ["git: file-write-create /Users/Shared/moor-n/a/home/x"]
    );
    assert_eq!(v.setup_denials.unwrap(), Vec::<String>::new());
}

#[test]
fn setup_and_live_denials_are_listed_apart() {
    let out = "moor-denials: report\n\
        2026-01-01 00:00:00.0 Sandbox: npm(7) deny(1) file-write-create /Users/Shared/moor-n/a/home/s\n\
        moor-denials: log\n\
        2026-01-01 00:00:01.0 Sandbox: git(42) deny(1) file-write-create /Users/Shared/moor-n/a/home/x\n";
    let (v, _) = show(FakeDriver::new().work_root(MAC_ROOT).exec_stdout(["", out]));
    let setup = v.setup_denials.unwrap();
    let live = v.sandbox_denials.unwrap();
    assert_eq!((setup.len(), live.len()), (1, 1), "{setup:?} {live:?}");
    let (s, l) = (setup.concat(), live.concat());
    assert!(s.contains("npm") && s.ends_with("home/s"), "{s}");
    assert!(l.contains("git") && l.ends_with("home/x"), "{l}");
}

#[test]
fn denial_text_is_cleaned_redacted_and_capped() {
    const NONCE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde";
    let long = "p".repeat(10_000);
    let out = format!(
        "moor-denials: log\n\
        2026-01-01 00:00:00.0 Sandbox: git(42) deny(1) file-write-create \
        \x1b[31m/a\x1b]0;t\x07/{NONCE}/{long}\n"
    );
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .work_root(MAC_ROOT)
            .exec_stdout(["", out.as_str()]),
    );
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    let reserved =
        cx.store()
            .unwrap()
            .reserve(id, &nonce, cx.rng(), crate::proxy::supervisor::LEASE);
    assert!(reserved.is_ok());
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let live = v.sandbox_denials.unwrap();
    assert_eq!(live.len(), 1);
    for line in &live {
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(!line.contains(NONCE), "{line:?}");
        assert!(line.chars().count() <= DENIAL_LINE_CHARS);
    }
}

#[test]
fn linux_task_has_no_denials_and_no_extra_exec() {
    let (v, driver) = show(FakeDriver::new());
    assert!(v.sandbox_denials.is_none());
    assert_eq!(driver.execs().len(), 1);
}

#[test]
fn exec_failure_leaves_denials_none() {
    let (v, _) = show(FakeDriver::new().work_root(MAC_ROOT).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Err(DriverError::NotFound),
    ]));
    assert!(v.sandbox_denials.is_none() && v.task.is_some());
}

#[test]
fn nonzero_exit_leaves_denials_none() {
    let (v, _) = show(FakeDriver::new().work_root(MAC_ROOT).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 1 }),
    ]));
    assert!(v.sandbox_denials.is_none());
}

const SPLIT_NONCE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde";

/// Show task `a` with proxy nonce `SPLIT_NONCE`; both the setup and live lists get one denial per
/// target. Returns the view.
fn show_with_nonce(targets: &[String]) -> moor_core::view::ShowView {
    let line = |t: &String| {
        format!("2026-01-01 00:00:00.0 Sandbox: git(42) deny(1) file-write-create /{t}\n")
    };
    let mut out = String::from("moor-denials: report\n");
    out.extend(targets.iter().map(line));
    out.push_str("moor-denials: log\n");
    out.extend(targets.iter().map(line));
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .work_root(MAC_ROOT)
            .exec_stdout(["", out.as_str()]),
    );
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let nonce = Secret::new(SPLIT_NONCE.as_bytes().to_vec());
    let reserved =
        cx.store()
            .unwrap()
            .reserve(id, &nonce, cx.rng(), crate::proxy::supervisor::LEASE);
    assert!(reserved.is_ok());
    run(&cx, &req(Some("a"), None)).unwrap()
}

#[test]
fn nonce_split_by_junk_is_still_redacted() {
    let (a, b) = SPLIT_NONCE.split_at(3);
    let targets = [format!("{a}\x01{b}"), format!("{a}\x1b[0m{b}")];
    let v = show_with_nonce(&targets);
    let json = serde_json::to_string(&v).unwrap();
    assert!(!json.contains(SPLIT_NONCE), "{json}");
    for list in [v.sandbox_denials.unwrap(), v.setup_denials.unwrap()] {
        assert_eq!(list.len(), 2, "{list:?}");
        for line in &list {
            assert!(!line.contains(SPLIT_NONCE), "{line:?}");
            assert!(line.contains(super::super::REDACTED), "{line:?}");
        }
    }
}

#[test]
fn nonce_straddling_the_cap_leaves_no_long_fragment() {
    let target = format!("{}{SPLIT_NONCE}", "p".repeat(DENIAL_LINE_CHARS - 10));
    let v = show_with_nonce(&[target]);
    let chars: Vec<char> = SPLIT_NONCE.chars().collect();
    for list in [v.sandbox_denials.unwrap(), v.setup_denials.unwrap()] {
        assert_eq!(list.len(), 1, "{list:?}");
        let line = list.first().unwrap();
        assert!(line.chars().count() <= DENIAL_LINE_CHARS, "{line:?}");
        for w in chars.windows(8) {
            let frag: String = w.iter().collect();
            assert!(!line.contains(&frag), "{frag} in {line:?}");
        }
    }
}
