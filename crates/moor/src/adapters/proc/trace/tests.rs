//! Tests for crabbox call tracing.

use std::ffi::OsString;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::{done_line, emit, exit_label, started_line, subcommand, Trace};
use crate::env::Env;

fn args(a: &[&str]) -> Vec<OsString> {
    a.iter().map(OsString::from).collect()
}

fn spec(tool: Tool) -> ProcSpec {
    ProcSpec::new(tool, Deadline(Duration::from_secs(1)))
}

fn trace(pairs: &[(&str, &str)]) -> Trace {
    let env = Env::from_pairs(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    Trace::new(&env)
}

#[test]
fn subcommand_skips_flags() {
    assert_eq!(
        subcommand(&args(&["--json", "--quiet", "warmup"])),
        "warmup"
    );
    assert_eq!(subcommand(&args(&["--json", "list"])), "list");
    assert_eq!(
        subcommand(&args(&["run", "--id", "abc", "--", "ls"])),
        "run"
    );
}

#[test]
fn subcommand_version_and_empty() {
    assert_eq!(subcommand(&args(&["--version"])), "--version");
    assert_eq!(subcommand(&args(&["--help"])), "-");
    assert_eq!(subcommand(&[]), "-");
}

#[test]
fn done_line_format_hides_other_args() {
    let line = done_line(
        &args(&["ssh", "--token", "s3cret"]),
        Duration::from_millis(1234),
        "0",
    );
    assert_eq!(line, "moor-trace: crabbox ssh took=1.234s exit=0");
    assert!(!line.contains("s3cret"));
}

#[test]
fn started_line_format() {
    assert_eq!(
        started_line(&args(&["tunnel", "--key", "k"])),
        "moor-trace: crabbox tunnel started"
    );
}

#[test]
fn exit_labels() {
    let nz = ProcError::NonZero {
        code: 3,
        stdout: String::new(),
        stderr_tail: String::new(),
    };
    assert_eq!(exit_label(None), "0");
    assert_eq!(exit_label(Some(&nz)), "3");
    assert_eq!(exit_label(Some(&ProcError::Signalled(9))), "signal");
    let spawn = ProcError::Spawn(std::io::Error::other("x"));
    assert_eq!(exit_label(Some(&spawn)), "spawn-error");
}

#[test]
fn gate_needs_var_set_to_one_and_crabbox_tool() {
    let on = trace(&[("MOOR_TRACE_CRABBOX", "1")]);
    assert!(on.active(&spec(Tool::Crabbox)));
    assert!(!on.active(&spec(Tool::Git)));
    assert!(!trace(&[]).active(&spec(Tool::Crabbox)));
    assert!(!trace(&[("MOOR_TRACE_CRABBOX", "0")]).active(&spec(Tool::Crabbox)));
    assert!(!trace(&[("MOOR_TRACE_CRABBOX", "")]).active(&spec(Tool::Crabbox)));
}

#[test]
fn timed_passes_result_through_either_way() {
    let off = trace(&[]);
    assert_eq!(off.timed(&spec(Tool::Crabbox), || Ok(7)).unwrap(), 7);
    let on = trace(&[("MOOR_TRACE_CRABBOX", "1")]);
    let err = on
        .timed::<()>(&spec(Tool::Crabbox), || Err(ProcError::Interrupted))
        .unwrap_err();
    assert!(matches!(err, ProcError::Interrupted));
}

#[test]
fn emit_ignores_write_failure() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    emit(&mut Broken, "x");
    let mut buf = Vec::new();
    emit(&mut buf, "x");
    assert_eq!(buf, b"x\n");
}
