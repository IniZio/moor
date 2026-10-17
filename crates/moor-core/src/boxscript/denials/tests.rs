//! Tests for sandbox-denial parsing and the denial-reading script.

use std::process::Command;

use super::{from_report, parse, script, window_script, Denial, DENIAL_LIMIT, PID_FILTER_AWK};

const NONO: &str = r#"{
  "session": {
    "exit_code": 1,
    "denials": [ { "path": "/Users/newman/.ssh/known_hosts", "access": "Read", "reason": "PolicyBlocked" } ],
    "ipc_denials": [],
    "violations": [ { "operation": "file-read-data", "target": "/Users/newman/.ssh/known_hosts" } ],
    "diagnostics": [ { "code": "sandbox_denied_path", "severity": "warning", "message": "x" } ]
  }
}
"#;

const LOG: &str = "2026-10-17 18:58:50.094 E  kernel[0:c2a7956] [com.apple.sandbox.reporting:violation] Sandbox: cat(73134) deny(1) file-read-data /Users/newman/.ssh/known_hosts";

fn den(op: &str, target: &str) -> Denial {
    Denial {
        process: None,
        operation: op.into(),
        target: target.into(),
    }
}

#[test]
fn report_sample_gives_one_denial() {
    let r = from_report(NONO);
    assert_eq!(
        r.denials,
        vec![den("file-read-data", "/Users/newman/.ssh/known_hosts")]
    );
    assert_eq!(r.text, "");
}

#[test]
fn report_keeps_leading_text_and_denials_without_violation() {
    let json = r#"{"session":{"denials":[{"path":"/a","access":"Write"}],"violations":[]}}"#;
    let r = from_report(&format!("nono: boom\n{json}\n"));
    assert_eq!(r.text, "nono: boom");
    assert_eq!(r.denials, vec![den("file-write", "/a")]);
}

#[test]
fn report_garbage_keeps_text() {
    let r = from_report("  not json {\n");
    assert!(r.denials.is_empty());
    assert_eq!(r.text, "not json {");
}

#[test]
fn report_dedupes_and_caps() {
    let v: Vec<String> = (0..30)
        .map(|i| format!(r#"{{"operation":"op","target":"/t{}"}}"#, i % 25))
        .collect();
    let json = format!(r#"{{"session":{{"violations":[{}]}}}}"#, v.join(","));
    let r = from_report(&json);
    assert_eq!(r.denials.len(), DENIAL_LIMIT);
    assert_eq!(r.denials.first(), Some(&den("op", "/t0")));
}

#[test]
fn parse_log_line() {
    let out = parse(&format!("moor-denials: report\nmoor-denials: log\n{LOG}\n"));
    assert_eq!(
        out,
        vec![Denial {
            process: Some("cat".into()),
            operation: "file-read-data".into(),
            target: "/Users/newman/.ssh/known_hosts".into(),
        }]
    );
    let shown: Vec<String> = out.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        ["cat: file-read-data /Users/newman/.ssh/known_hosts"]
    );
}

#[test]
fn parse_mixed_sections_dedupes_keeping_latest() {
    let out = parse(&format!(
        "moor-denials: report\n{NONO}moor-denials: log\n{LOG}\n{LOG}\n"
    ));
    assert_eq!(out.len(), 2);
    let procs: Vec<Option<&str>> = out.iter().map(|d| d.process.as_deref()).collect();
    assert_eq!(procs, [None, Some("cat")]);
}

#[test]
fn parse_caps_to_last() {
    let lines: Vec<String> = (0..30)
        .map(|i| format!("x Sandbox: p({i}) deny(1) file-read-data /t{i}"))
        .collect();
    let out = parse(&format!("moor-denials: log\n{}\n", lines.join("\n")));
    let targets: Vec<&str> = out.iter().map(|d| d.target.as_str()).collect();
    assert_eq!(targets.len(), DENIAL_LIMIT);
    assert_eq!(targets.first(), Some(&"/t10"));
    assert_eq!(targets.last(), Some(&"/t29"));
}

#[test]
fn script_has_parts_and_valid_syntax() {
    let s = script("/r/it's", "/w", 30);
    assert!(s.contains("'/r/it'\\''s'"));
    assert!(s.contains("'/w/.moor/setup.sandbox.json'"));
    assert!(s.contains(r#"--predicate 'subsystem == "com.apple.sandbox.reporting"'"#));
    assert!(s.contains("tail -n 200"));
    assert!(s.contains("--last 30m"));
    for line in s.lines().filter(|l| l.trim_start().starts_with("case ")) {
        assert!(line.contains(" in ("), "{line}");
    }
    let ok = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&s)
        .status()
        .unwrap();
    assert!(ok.success());
}

#[test]
fn script_runs_quietly_without_nono() {
    let out = Command::new("sh")
        .arg("-c")
        .arg(script("/nonexistent-moor-root", "/nonexistent-workdir", 5))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "moor-denials: report\nmoor-denials: log\n"
    );
}

#[test]
fn script_report_section_reads_saved_denials() {
    let s = script("/r", "/w", 5);
    assert!(s.contains("cat '/w/.moor/setup.denials'"));
}

#[test]
fn window_script_has_parts_and_valid_syntax() {
    let s = window_script("/w/it's");
    assert!(s.contains("sed -n 1p '/w/it'\\''s/.moor/setup.sandbox.start'"));
    assert!(s.contains("sed -n 2p '/w/it'\\''s/.moor/setup.sandbox.start'"));
    assert!(s.contains(r#"awk -v minpid="$p""#));
    assert!(s.contains(r#"--start "$s""#));
    assert!(s.contains(r#"--predicate 'subsystem == "com.apple.sandbox.reporting"'"#));
    assert!(s.contains("grep ' deny('"));
    assert!(
        s.contains("> '/w/it'\\''s/.moor/setup.denials'; cat '/w/it'\\''s/.moor/setup.denials'")
    );
    let ok = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&s)
        .status()
        .unwrap();
    assert!(ok.success());
}

fn filter(minpid: &str, input: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("awk")
        .args(["-v", &format!("minpid={minpid}"), PID_FILTER_AWK])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
}

const TWO: &str = "a Sandbox: IMTransferAgent(412) deny(1) file-read-data /x\nb Sandbox: cat(73134) deny(1) file-read-data /y\n";

#[test]
fn pid_filter_keeps_only_pids_from_start() {
    let out = filter("70000", TWO);
    assert!(!out.contains("(412)"));
    assert!(out.contains("(73134)"));
    assert_eq!(filter("73134", TWO).lines().count(), 1);
}

#[test]
fn pid_filter_keeps_all_without_pid() {
    assert_eq!(filter("0", TWO), TWO);
}

#[test]
fn window_script_treats_bad_pid_as_none() {
    let s = window_script("/w");
    let case = s.lines().find(|l| l.starts_with("case ")).unwrap();
    assert!(case.contains(" in (''|*[!0-9]*) p=0"), "{case}");
}

#[test]
fn window_script_is_quiet_without_start_file() {
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(window_script("/nonexistent-moor-workdir"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn parse_raw_lines_without_markers() {
    let out = parse(&format!("{LOG}\n"));
    assert_eq!(out.len(), 1);
    assert_eq!(out.first().unwrap().operation, "file-read-data");
    let out = parse(&format!("moor-denials: report\n{LOG}\nmoor-denials: log\n"));
    assert_eq!(out.len(), 1);
}

#[test]
fn parse_report_with_json_and_log_lines() {
    let out = parse(&format!(
        "moor-denials: report\n{NONO}{LOG}\nmoor-denials: log\n"
    ));
    assert_eq!(out.len(), 2);
}

#[test]
fn parse_keeps_blocking_denial_among_noise() {
    let mut lines = vec![LOG.to_owned()];
    lines.extend(
        (0..50).map(|i| format!("x Sandbox: p({i}) deny(1) user-preference-read com.apple.n{i}")),
    );
    let out = parse(&lines.join("\n"));
    assert_eq!(out.len(), DENIAL_LIMIT);
    assert_eq!(out.first().unwrap().operation, "file-read-data");
    assert_eq!(out.last().unwrap().target, "com.apple.n49");
}
