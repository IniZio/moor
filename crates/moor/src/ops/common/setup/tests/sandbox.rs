use super::*;
use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::engine::Sandboxed;

use super::super::denials;
use crate::testing::engine::FakeEngine;

fn failed(driver: FakeDriver) -> String {
    failed_with(driver, exits(&[0, 0, 1])).0
}

/// The warning, and the argv of every exec the failed setup made.
fn failed_with(
    driver: FakeDriver,
    results: Vec<Result<ExecOutcome, DriverError>>,
) -> (String, Vec<Vec<String>>) {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), driver.exec_results(results));
    let engine = FakeEngine::new().sandbox_result(Ok(Some(Sandboxed {
        lines: vec![],
        prefix: vec!["nono".to_owned(), "run".to_owned(), "--".to_owned()],
    })));
    r.cx = r.cx.with_engine(Arc::new(engine) as Arc<_>);
    r.cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    let workdir = BoxPath::restore("/Users/box/moor/demo/work/repo").unwrap();
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
        None,
        TaskId::new(1),
    );
    let argvs = r.driver.execs().into_iter().map(|e| e.1.argv).collect();
    (only(r.cx.take_warnings()).message, argvs)
}

const KNOWN_HOSTS: &str = "2026-10-17 18:58:50.094 E  kernel[0:c2a7956] [com.apple.sandbox.reporting:violation] Sandbox: cat(73134) deny(1) file-read-data /Users/newman/.ssh/known_hosts";

#[test]
fn a_child_process_denial_from_the_box_log_is_named() {
    let (msg, argvs) = failed_with(
        FakeDriver::new().exec_stdout([KNOWN_HOSTS]),
        exits(&[0, 0, 1, 0]),
    );
    assert!(
        msg.contains("\nSeatbelt denials logged on the Box since setup started"),
        "{msg}"
    );
    assert!(
        msg.contains("\n  cat: file-read-data /Users/newman/.ssh/known_hosts"),
        "{msg}"
    );
    let window = argvs.last().unwrap();
    assert!(
        window.get(2).unwrap().contains(".moor/setup.sandbox.start"),
        "{window:?}"
    );
}

#[test]
fn a_failed_window_exec_leaves_no_denial_lines() {
    let mut results = exits(&[0, 0, 1]);
    results.push(Err(DriverError::NotFound));
    let msg = failed_with(FakeDriver::new().exec_stdout([KNOWN_HOSTS]), results).0;
    assert!(!msg.contains("Seatbelt denials"), "{msg}");
    assert!(msg.starts_with("setup line `a` failed: exit 1"), "{msg}");
}

#[test]
fn a_root_process_denial_in_the_report_joins_the_box_log() {
    let msg = failed(
        FakeDriver::new()
            .exec_stdout([KNOWN_HOSTS])
            .copy_report(report(1)),
    );
    assert!(
        msg.contains("\n  cat: file-read-data /Users/newman/.ssh/known_hosts"),
        "{msg}"
    );
    assert!(
        msg.contains("\n  file-read-data /Users/x/.ssh/key0"),
        "{msg}"
    );
}

fn report(n: usize) -> String {
    let v: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"operation":"file-read-data","target":"/Users/x/.ssh/key{i}"}}"#))
        .collect();
    format!(r#"{{"session":{{"violations":[{}]}}}}"#, v.join(","))
}

#[test]
fn a_denial_is_named_right_after_the_first_line() {
    let msg = failed(FakeDriver::new().copy_report(report(1)));
    let (first, rest) = msg.split_once('\n').unwrap();
    assert!(first.starts_with("setup line `a` failed: exit 1"), "{msg}");
    assert!(
        rest.starts_with(
            "Seatbelt denials logged on the Box since setup started (may include other processes):\n  file-read-data /Users/x/.ssh/key0\nfull log in the Box"
        ),
        "{msg}"
    );
}

#[test]
fn a_report_with_only_text_shows_its_first_five_lines() {
    let text = (1..=8)
        .map(|i| format!("nono: line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let msg = failed(FakeDriver::new().copy_report(text));
    assert!(
        msg.contains("\nsandbox: nono: line 1\nnono: line 2\n"),
        "{msg}"
    );
    assert!(msg.contains("nono: line 5"), "{msg}");
    assert!(!msg.contains("nono: line 6"), "{msg}");
}

#[test]
fn no_report_leaves_the_message_unchanged() {
    let msg = failed(FakeDriver::new());
    assert!(!msg.contains("sandbox"), "{msg}");
    assert!(
        msg.starts_with("setup line `a` failed: exit 1\nfull log in the Box"),
        "{msg}"
    );
}

#[test]
fn more_than_ten_denials_show_the_newest_ten_after_the_earlier_count() {
    let msg = failed(FakeDriver::new().copy_report(report(12)));
    assert_eq!(msg.matches("\n  file-read-data ").count(), 10, "{msg}");
    assert!(
        msg.contains("processes):\n  … and 2 earlier\n  file-read-data /Users/x/.ssh/key2\n"),
        "{msg}"
    );
    assert!(msg.contains("key11\n"), "{msg}");
    assert!(!msg.contains("key1\n"), "{msg}");
}

#[test]
fn the_newest_denial_is_shown_among_many() {
    let msg = failed(FakeDriver::new().copy_report(report(25)));
    assert!(msg.contains("\n  … and 10 earlier\n"), "{msg}");
    assert!(
        msg.contains("\n  file-read-data /Users/x/.ssh/key19\n"),
        "{msg}"
    );
}

#[test]
fn a_planted_denial_line_loses_escapes_and_nonce_and_is_capped() {
    let nonce = "q7v2k9x4q7v2k9x4q7v2";
    let line = format!(
        "{}\x1b[31m{nonce}{}",
        KNOWN_HOSTS.replace("/Users/newman/.ssh/known_hosts", "/p/"),
        "a".repeat(10_000)
    );
    let found = moor_core::boxscript::denials::parse(&line);
    assert_eq!(found.len(), 1, "{line}");
    let msg = denials::describe(&found, "", Some(nonce.as_bytes()));
    assert!(!msg.contains('\x1b'), "{msg}");
    assert!(!msg.contains(nonce), "{msg}");
    assert!(msg.len() < 2_000, "{}", msg.len());
    let text = denials::describe(
        &[],
        &format!("nono: \x1b[31m{nonce}"),
        Some(nonce.as_bytes()),
    );
    assert!(!text.contains('\x1b') && !text.contains(nonce), "{text}");
}

const SPLIT_NONCE: &str = "q7v2k9x4m5n6p8r3t1w0";

fn shows_nonce_part(msg: &str) -> bool {
    let n: Vec<char> = SPLIT_NONCE.chars().collect();
    n.windows(8)
        .any(|w| msg.contains(&w.iter().collect::<String>()))
}

fn denial_line(target: &str) -> String {
    KNOWN_HOSTS.replace("/Users/newman/.ssh/known_hosts", target)
}

fn describe_denial(target: &str) -> String {
    let found = moor_core::boxscript::denials::parse(&denial_line(target));
    assert_eq!(found.len(), 1, "{target}");
    denials::describe(&found, "", Some(SPLIT_NONCE.as_bytes()))
}

fn describe_text(text: &str) -> String {
    denials::describe(&[], text, Some(SPLIT_NONCE.as_bytes()))
}

fn straddling(prefix_chars: usize) -> String {
    format!("{}{SPLIT_NONCE}tail", "/p".repeat(prefix_chars / 2))
}

#[test]
fn a_nonce_straddling_the_cap_in_a_denial_is_not_shown() {
    for prefix in 270..310 {
        let msg = describe_denial(&straddling(prefix));
        assert!(!shows_nonce_part(&msg), "{prefix}: {msg}");
        let line = msg.lines().last().unwrap().trim_start();
        assert!(line.chars().count() <= 300, "{prefix}: {}", line.len());
    }
}

#[test]
fn a_nonce_split_by_a_control_char_in_a_denial_is_not_shown() {
    for mid in ["\x01", "\x1b[0m"] {
        let (a, b) = SPLIT_NONCE.split_at(9);
        let msg = describe_denial(&format!("/p/{a}{mid}{b}"));
        assert!(!shows_nonce_part(&msg), "{msg:?}");
        assert!(!msg.chars().any(|c| c == '\x1b' || c == '\x01'), "{msg:?}");
    }
}

#[test]
fn a_nonce_straddling_the_cap_in_sandbox_text_is_not_shown() {
    for prefix in 270..310 {
        let msg = describe_text(&straddling(prefix));
        assert!(!shows_nonce_part(&msg), "{prefix}: {msg}");
        let line = msg.strip_prefix("\nsandbox: ").unwrap();
        assert!(line.chars().count() <= 300, "{prefix}: {}", line.len());
    }
}

#[test]
fn a_nonce_split_by_a_control_char_in_sandbox_text_is_not_shown() {
    for mid in ["\x01", "\x1b[0m"] {
        let (a, b) = SPLIT_NONCE.split_at(9);
        let msg = describe_text(&format!("nono: {a}{mid}{b}"));
        assert!(!shows_nonce_part(&msg), "{msg:?}");
        assert!(!msg.chars().any(|c| c == '\x1b' || c == '\x01'), "{msg:?}");
    }
}
