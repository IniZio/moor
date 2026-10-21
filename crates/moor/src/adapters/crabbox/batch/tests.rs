use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{BoxStep, Driver, DriverError, ExecOutcome, StepDone};
use moor_core::seam::proc::{Deadline, Tool};

use super::{script, stdin_bytes};
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

type Results = Vec<Result<StepDone, DriverError>>;

const SECRET: &[u8] = b"s3cret-value";

fn connect() -> Vec<String> {
    ["ssh", "-t", "-i", "/k/id", "-p", "22", "u@10.0.0.1"]
        .map(str::to_owned)
        .to_vec()
}

fn path(p: &str) -> BoxPath {
    BoxPath::restore(p).unwrap()
}

fn put(p: &str, bytes: &[u8], mode: u32) -> BoxStep {
    BoxStep::Put {
        path: path(p),
        bytes: bytes.to_vec(),
        mode,
    }
}

fn batch(p: &Arc<ScriptedProc>, connect: &[String], steps: &[BoxStep]) -> Results {
    Crabbox::fake(p.clone()).batch(
        &BoxHandle::new("p/box"),
        connect,
        steps,
        Deadline(Duration::from_secs(60)),
    )
}

fn scripted(reply: Reply) -> Arc<ScriptedProc> {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Ssh, &[], reply);
    p
}

fn failed(summary: &str) -> DriverError {
    DriverError::Failed {
        summary: summary.to_owned(),
    }
}

fn ran(code: i32, stdout: &str) -> StepDone {
    StepDone::Ran {
        outcome: ExecOutcome::Exited { code },
        stdout: stdout.to_owned(),
    }
}

#[test]
fn one_ssh_process_with_the_connect_target_and_no_tty_flag() {
    let p = scripted(Reply::Ok("A\nO\n".to_owned()));
    let steps = [BoxStep::Peek(path("/w/a")), put("/w/b", b"x", 0o600)];
    let out = batch(&p, &connect(), &steps);
    assert_eq!(out, [Ok(StepDone::Peeked(None)), Ok(StepDone::Put)]);
    let [call] = p.calls().try_into().unwrap();
    assert_eq!(call.tool, Tool::Ssh);
    let [i, k, port, n, target, remote] = call.args.try_into().unwrap();
    assert_eq!(
        [i, k, port, n, target],
        ["-i", "/k/id", "-p", "22", "u@10.0.0.1"]
    );
    assert!(remote.starts_with("sh -c '"), "{remote}");
}

#[test]
fn connect_without_ssh_falls_back_to_one_lookup() {
    let p = scripted(Reply::Ok("A\n".to_owned()));
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("ssh -i '/k/id' 'u@10.0.0.1'\n".to_owned()),
    );
    batch(&p, &[], &[BoxStep::Peek(path("/w/a"))]);
    let [lookup, ssh] = p.calls().try_into().unwrap();
    assert_eq!(lookup.tool, Tool::Crabbox);
    assert_eq!(ssh.args.first().map(String::as_str), Some("-i"));
}

#[test]
fn no_steps_run_no_process() {
    let p = scripted(Reply::Ok(String::new()));
    assert!(batch(&p, &connect(), &[]).is_empty());
    assert!(p.calls().is_empty());
}

#[test]
fn put_bytes_ride_stdin_and_never_argv() {
    let p = scripted(Reply::Ok("O\nO\n".to_owned()));
    let steps = [put("/w/a", SECRET, 0o600), put("/w/b", b"tail", 0o644)];
    batch(&p, &connect(), &steps);
    let calls = p.calls();
    assert!(calls.iter().all(|c| c.stdin == b"s3cret-valuetail"));
    for c in &calls {
        assert!(c.args.iter().all(|a| !a.contains("s3cret-value")), "{c:?}");
    }
}

#[test]
fn frames_become_results_in_step_order() {
    let reply = "P 5\nhello\nA\nR 0 3\nout\nO\n";
    let p = scripted(Reply::Ok(reply.to_owned()));
    let steps = [
        BoxStep::Peek(path("/w/a")),
        BoxStep::Peek(path("/w/b")),
        BoxStep::Run("echo out".to_owned()),
        put("/w/c", b"", 0o600),
    ];
    assert_eq!(
        batch(&p, &connect(), &steps),
        [
            Ok(StepDone::Peeked(Some("hello".to_owned()))),
            Ok(StepDone::Peeked(None)),
            Ok(ran(0, "out")),
            Ok(StepDone::Put),
        ]
    );
}

#[test]
fn a_non_zero_run_stops_the_batch() {
    let p = scripted(Reply::Ok("R 3 0\n\n".to_owned()));
    let steps = [
        BoxStep::Run("exit 3".to_owned()),
        BoxStep::Peek(path("/w/a")),
    ];
    assert_eq!(batch(&p, &connect(), &steps), [Ok(ran(3, ""))]);
}

#[test]
fn a_failed_write_frame_is_a_write_error_and_stops() {
    let p = scripted(Reply::Ok("O\nF\n".to_owned()));
    let steps = [
        put("/w/a", b"1", 0o600),
        put("/w/b", b"2", 0o600),
        BoxStep::Run("true".to_owned()),
    ];
    assert_eq!(
        batch(&p, &connect(), &steps),
        [Ok(StepDone::Put), Err(failed("box file write failed"))]
    );
}

#[test]
fn a_read_error_frame_is_a_read_error() {
    let p = scripted(Reply::Ok("E\n".to_owned()));
    let out = batch(&p, &connect(), &[BoxStep::Peek(path("/w/a"))]);
    assert_eq!(out, [Err(failed("box file read failed"))]);
}

#[test]
fn ssh_failure_adds_one_error_for_the_first_unframed_step() {
    let p = scripted(Reply::Fail {
        code: 255,
        stdout: String::new(),
        stderr: "lost".to_owned(),
    });
    let steps = [BoxStep::Run("a".to_owned()), BoxStep::Peek(path("/w/a"))];
    assert_eq!(
        batch(&p, &connect(), &steps),
        [Err(failed("box command failed"))]
    );
    let steps = [BoxStep::Peek(path("/w/a")), put("/w/b", b"1", 0o600)];
    assert_eq!(
        batch(&p, &connect(), &steps),
        [Err(failed("box file read failed"))]
    );
}

#[test]
fn an_ssh_drop_is_a_lost_connection_not_a_step_failure() {
    let p = scripted(Reply::Fail {
        code: 255,
        stdout: String::new(),
        stderr: "client_loop: send disconnect: Broken pipe\n".to_owned(),
    });
    assert_eq!(
        batch(&p, &connect(), &[put("/w/a", b"1", 0o600)]),
        [Err(DriverError::Transport {
            stderr_tail: "client_loop: send disconnect: Broken pipe".to_owned(),
        })]
    );
}

#[test]
fn ssh_timeout_is_a_timeout() {
    let p = scripted(Reply::Timeout);
    let out = batch(&p, &connect(), &[put("/w/a", b"1", 0o600)]);
    assert!(
        matches!(out.as_slice(), [Err(DriverError::Timeout(_))]),
        "{out:?}"
    );
}

#[test]
fn garbled_output_fails_the_unframed_step() {
    let p = scripted(Reply::Ok("P 99\nshort\n".to_owned()));
    let out = batch(&p, &connect(), &[BoxStep::Peek(path("/w/a"))]);
    assert_eq!(out, [Err(failed("box file read failed"))]);
}

/// Runs the generated script under `/bin/sh` the way ssh would.
fn run_for_real(steps: &[BoxStep]) -> Vec<u8> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(script(steps))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let bytes = stdin_bytes(steps);
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let _ = stdin.write_all(&bytes);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(out.status.success());
    out.stdout
}

fn mode_of(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn real_sh_writes_files_with_modes_and_frames_peeks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    let f = |n: &str| format!("{root}/sub/{n}");
    let nul: &[u8] = b"a\0b\n\0";
    let steps = [
        put(&f("a"), b"first\n", 0o600),
        put(&f("b"), nul, 0o644),
        put(&f("c"), b"", 0o700),
        put(&f("d"), b"after", 0o600),
        BoxStep::Peek(path(&f("b"))),
        BoxStep::Peek(path(&f("missing"))),
        BoxStep::Run(format!("printf 'x\\ny' && cat {}", f("a"))),
    ];
    let out = run_for_real(&steps);
    let results = super::parse(&out, &steps);
    assert_eq!(
        results,
        [
            Ok(StepDone::Put),
            Ok(StepDone::Put),
            Ok(StepDone::Put),
            Ok(StepDone::Put),
            Ok(StepDone::Peeked(Some("a\0b\n\0".to_owned()))),
            Ok(StepDone::Peeked(None)),
            Ok(ran(0, "x\nyfirst\n")),
        ]
    );
    assert_eq!(std::fs::read(f("a")).unwrap(), b"first\n");
    assert_eq!(std::fs::read(f("b")).unwrap(), nul);
    assert_eq!(std::fs::read(f("c")).unwrap(), b"");
    assert_eq!(std::fs::read(f("d")).unwrap(), b"after");
    assert_eq!(mode_of(Path::new(&f("a"))), 0o600);
    assert_eq!(mode_of(Path::new(&f("b"))), 0o644);
    assert_eq!(mode_of(Path::new(&f("c"))), 0o700);
}

#[test]
fn real_sh_stops_after_a_failing_run() {
    let dir = tempfile::tempdir().unwrap();
    let marker = format!("{}/never", dir.path().display());
    let steps = [
        BoxStep::Run("echo hi; exit 4".to_owned()),
        put(&marker, b"x", 0o600),
    ];
    let out = run_for_real(&steps);
    assert_eq!(super::parse(&out, &steps), [Ok(ran(4, "hi\n"))]);
    assert!(!Path::new(&marker).exists());
}

#[test]
fn script_never_reads_stdin_byte_by_byte() {
    let steps = [put("/w/a", b"x", 0o600), BoxStep::Run("true".to_owned())];
    assert!(!script(&steps).contains("dd bs=1"));
}

#[test]
fn steps_without_a_put_never_read_stdin() {
    let steps = [BoxStep::Peek(path("/w/a")), BoxStep::Run("true".to_owned())];
    assert!(!script(&steps).contains("cat >"));
}

#[test]
fn real_sh_slices_large_puts_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    let f = |n: &str| format!("{root}/{n}");
    let blob = |seed: usize, len: usize| -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from((i * 31 + seed * 7 + i / 251) % 256).unwrap())
            .collect()
    };
    let (a, b, c) = (blob(1, 200_000), blob(2, 200_001), blob(3, 77));
    let steps = [
        put(&f("a"), &a, 0o600),
        put(&f("b"), &b, 0o644),
        put(&f("c"), &c, 0o600),
    ];
    let out = run_for_real(&steps);
    assert_eq!(
        super::parse(&out, &steps),
        [Ok(StepDone::Put), Ok(StepDone::Put), Ok(StepDone::Put)]
    );
    assert_eq!(std::fs::read(f("a")).unwrap(), a);
    assert_eq!(std::fs::read(f("b")).unwrap(), b);
    assert_eq!(std::fs::read(f("c")).unwrap(), c);
}
