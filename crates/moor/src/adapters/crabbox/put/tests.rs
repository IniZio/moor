use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Call, Reply, ScriptedProc};

const SSH_LINE: &str = "ssh -i '/k/id' -o 'StrictHostKeyChecking=yes' -p '22' 'u@10.0.0.1'\n";
const SECRET: &[u8] = b"token-123 it's $(x)\n";

fn put(p: &Arc<ScriptedProc>, path: &str) -> Result<(), DriverError> {
    Crabbox::fake(p.clone()).put_file(
        &BoxHandle::new("p/box"),
        &BoxPath::restore(path).unwrap(),
        SECRET,
        0o600,
    )
}

fn scripted(write: Reply) -> Arc<ScriptedProc> {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    p.on(Tool::Ssh, &[], write);
    p
}

fn ssh_call(p: &ScriptedProc) -> Call {
    p.calls().into_iter().find(|c| c.tool == Tool::Ssh).unwrap()
}

#[test]
fn remote_command_makes_the_dir_writes_stdin_then_sets_mode() {
    let p = scripted(Reply::Ok(String::new()));
    put(&p, "/home/u/.moor-cred-x/brief.md").unwrap();
    let call = ssh_call(&p);
    assert_eq!(
        call.args,
        [
            "-i",
            "/k/id",
            "-o",
            "StrictHostKeyChecking=yes",
            "-p",
            "22",
            "u@10.0.0.1",
            "umask 077 && mkdir -p '/home/u/.moor-cred-x' && cat > '/home/u/.moor-cred-x/brief.md' && chmod 600 '/home/u/.moor-cred-x/brief.md'",
        ]
    );
    assert_eq!(call.stdin, SECRET);
}

#[test]
fn content_never_appears_in_any_argv() {
    let p = scripted(Reply::Ok(String::new()));
    put(&p, "/home/u/f").unwrap();
    for c in p.calls() {
        assert!(c.args.iter().all(|a| !a.contains("token-123")), "{c:?}");
    }
}

#[test]
fn path_is_quoted_for_the_remote_shell() {
    let p = scripted(Reply::Ok(String::new()));
    put(&p, "/h/it's a;b/f").unwrap();
    let cmd = ssh_call(&p).args.pop().unwrap();
    assert!(cmd.contains("mkdir -p '/h/it'\\''s a;b'"), "{cmd}");
    assert!(cmd.contains("cat > '/h/it'\\''s a;b/f'"), "{cmd}");
}

#[test]
fn file_at_the_root_creates_the_root() {
    let p = scripted(Reply::Ok(String::new()));
    put(&p, "/f").unwrap();
    assert!(ssh_call(&p).args.pop().unwrap().contains("mkdir -p '/' "));
}

#[test]
fn gone_box_is_not_found_without_running_ssh() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Fail {
            code: 4,
            stdout: String::new(),
            stderr: "lease not found".to_owned(),
        },
    );
    assert_eq!(put(&p, "/h/f"), Err(DriverError::NotFound));
    assert!(p.calls().iter().all(|c| c.tool != Tool::Ssh));
}

#[test]
fn hung_write_times_out_within_the_deadline() {
    let p = scripted(Reply::Timeout);
    let limit = Duration::from_secs(40);
    let err = put(&p, "/h/f").unwrap_err();
    assert!(
        matches!(err, DriverError::Timeout(d) if d <= limit),
        "{err:?}"
    );
    let total: Duration = p.calls().iter().map(|c| c.deadline.0).sum();
    assert!(total <= limit, "{total:?}");
}

#[test]
fn failed_write_hides_raw_stderr() {
    let p = scripted(Reply::Fail {
        code: 1,
        stdout: String::new(),
        stderr: "secret raw text".to_owned(),
    });
    let err = put(&p, "/h/f").unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }));
    assert!(!err.to_string().contains("secret raw text"));
}

const LOST: &str = "Connection reset by peer\n";

fn lost_then(rest: Vec<Reply>) -> Arc<ScriptedProc> {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    let rest = std::sync::Mutex::new(std::collections::VecDeque::from(rest));
    let next = move |_: &[String]| rest.lock().unwrap().pop_front().unwrap_or(Reply::Timeout);
    p.on(Tool::Ssh, &[], Reply::Computed(Arc::new(next)));
    p
}

fn ssh_calls(p: &ScriptedProc) -> usize {
    p.calls().iter().filter(|c| c.tool == Tool::Ssh).count()
}

fn lost() -> Reply {
    Reply::Fail {
        code: 255,
        stdout: String::new(),
        stderr: LOST.to_owned(),
    }
}

#[test]
fn a_lost_connection_is_retried_and_the_write_succeeds() {
    let p = lost_then(vec![lost(), lost(), Reply::Ok(String::new())]);
    put(&p, "/h/f").unwrap();
    assert_eq!(ssh_calls(&p), 3);
    for c in p.calls().iter().filter(|c| c.tool == Tool::Ssh) {
        assert_eq!(c.stdin, SECRET, "every try sends the whole content");
    }
}

#[test]
fn a_connection_lost_past_the_retries_is_a_transport_error() {
    let p = lost_then(vec![lost(), lost(), lost(), Reply::Ok(String::new())]);
    let err = put(&p, "/h/f").unwrap_err();
    assert!(matches!(err, DriverError::Transport { .. }), "{err:?}");
    assert_eq!(ssh_calls(&p), 3);
}

#[test]
fn a_failure_that_is_not_a_lost_connection_is_not_retried() {
    let p = lost_then(vec![
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "cat: write error".to_owned(),
        },
        Reply::Ok(String::new()),
    ]);
    let err = put(&p, "/h/f").unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    assert_eq!(ssh_calls(&p), 1);
}

/// Runs a put on a driver whose clock the caller keeps, and returns the time it slept.
fn slept_during_put(p: &Arc<ScriptedProc>) -> (Result<(), DriverError>, Duration) {
    use crate::testing::{clock::FakeClock, rng::FakeRng};
    use moor_core::seam::clock::{Clock as _, Ts};
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let driver =
        Crabbox::new(p.clone(), clock.clone(), Arc::new(FakeRng::default())).with_scratch_leases();
    driver.version_ok.set(()).unwrap();
    let put = driver.put_file(
        &BoxHandle::new("p/box"),
        &BoxPath::restore("/h/f").unwrap(),
        SECRET,
        0o600,
    );
    let slept = Duration::from_millis(clock.now().as_millis().try_into().unwrap());
    (put, slept)
}

#[test]
fn each_retry_waits_one_second() {
    let p = lost_then(vec![lost(), lost(), Reply::Ok(String::new())]);
    let (put, slept) = slept_during_put(&p);
    put.unwrap();
    assert_eq!(slept, Duration::from_secs(2), "two retries, 1 s each");
}

#[test]
fn a_put_that_needs_no_retry_does_not_wait() {
    let p = lost_then(vec![Reply::Ok(String::new())]);
    let (put, slept) = slept_during_put(&p);
    put.unwrap();
    assert_eq!(slept, Duration::ZERO);
}

#[test]
fn a_put_that_gives_up_waits_only_between_tries() {
    let p = lost_then(vec![lost(), lost(), lost()]);
    let (put, slept) = slept_during_put(&p);
    assert!(put.is_err());
    assert_eq!(slept, Duration::from_secs(2), "no pause after the last try");
}
