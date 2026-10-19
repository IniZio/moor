use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::{Output, Proc, ProcError, ProcOutput, ProcSpec, Tool};
use zeroize::Zeroizing;

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Call, Reply, ScriptedProc};

const SSH_LINE: &str = "ssh -i '/k/id' -o 'BatchMode=yes' -p '22' -o 'ProxyCommand=nc -x '\\''h p'\\'' %h' 'u@10.0.0.1'\n";

fn fixture() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn peek(c: &Crabbox, rel: &str) -> Result<Option<String>, DriverError> {
    c.peek_file(
        &BoxHandle::new("p/box"),
        &BoxPath::under("/work", rel).unwrap(),
        Path::new("/host/cwd"),
    )
}

fn call_of(p: &ScriptedProc, tool: Tool) -> Call {
    p.calls().into_iter().find(|c| c.tool == tool).unwrap()
}

fn scripted(p: &ScriptedProc, read: Reply) {
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    p.on(Tool::Ssh, &[], read);
}

#[test]
fn target_comes_from_crabbox_ssh_for_that_box() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    peek(&c, "a.json").unwrap();
    assert_eq!(
        call_of(&p, Tool::Crabbox).args,
        ["ssh", "--provider", "p", "--id", "box"]
    );
}

#[test]
fn ssh_gets_the_target_words_then_one_quoted_remote_command() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    peek(&c, "a.json").unwrap();
    assert_eq!(
        call_of(&p, Tool::Ssh).args,
        [
            "-i",
            "/k/id",
            "-o",
            "BatchMode=yes",
            "-p",
            "22",
            "-o",
            "ProxyCommand=nc -x 'h p' %h",
            "u@10.0.0.1",
            "test -e '/work/a.json' || exit 44; cat -- '/work/a.json'",
        ]
    );
}

#[test]
fn path_is_quoted_for_the_remote_shell() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok(String::new()));
    peek(&c, "it's $(x) a;b").unwrap();
    let args = call_of(&p, Tool::Ssh).args;
    let cmd = args.last().unwrap();
    assert_eq!(
        cmd,
        "test -e '/work/it'\\''s $(x) a;b' || exit 44; cat -- '/work/it'\\''s $(x) a;b'"
    );
}

#[test]
fn contents_are_returned() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("{\"a\":1}\n".to_owned()));
    assert_eq!(peek(&c, "a.json"), Ok(Some("{\"a\":1}\n".to_owned())));
}

#[test]
fn empty_file_is_some_empty() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok(String::new()));
    assert_eq!(peek(&c, "a.json"), Ok(Some(String::new())));
}

#[test]
fn missing_file_is_none() {
    let (p, c) = fixture();
    scripted(
        &p,
        Reply::Fail {
            code: 44,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    assert_eq!(peek(&c, "a.json"), Ok(None));
}

#[test]
fn unreadable_file_is_an_error_not_none() {
    let (p, c) = fixture();
    scripted(
        &p,
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "cat: denied".to_owned(),
        },
    );
    assert!(matches!(
        peek(&c, "a.json"),
        Err(DriverError::Failed { .. })
    ));
}

#[test]
fn hung_read_times_out_within_the_deadline() {
    let (p, c) = fixture();
    scripted(&p, Reply::Timeout);
    let limit = Duration::from_secs(10);
    let err = peek(&c, "a.json").unwrap_err();
    assert!(
        matches!(err, DriverError::Timeout(d) if d <= limit),
        "{err:?}"
    );
    let total: Duration = p.calls().iter().map(|c| c.deadline.0).sum();
    assert!(total <= limit, "{total:?}");
}

#[test]
fn unreachable_target_is_not_missing() {
    let (p, c) = fixture();
    scripted(
        &p,
        Reply::Fail {
            code: 255,
            stdout: String::new(),
            stderr: "ssh: connect refused".to_owned(),
        },
    );
    assert!(matches!(
        peek(&c, "a.json"),
        Err(DriverError::Failed { .. })
    ));
}

#[test]
fn gone_box_is_not_found() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Fail {
            code: 4,
            stdout: String::new(),
            stderr: "lease not found".to_owned(),
        },
    );
    assert_eq!(peek(&c, "a.json"), Err(DriverError::NotFound));
    assert!(p.calls().iter().all(|c| c.tool != Tool::Ssh));
}

#[test]
fn unreadable_target_line_is_an_error_without_running_ssh() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("'unterminated\n".to_owned()),
    );
    assert!(matches!(
        peek(&c, "a.json"),
        Err(DriverError::Failed { .. })
    ));
    assert!(p.calls().iter().all(|c| c.tool != Tool::Ssh));
}

#[test]
fn errors_hide_raw_stderr() {
    let (p, c) = fixture();
    scripted(
        &p,
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "secret raw text".to_owned(),
        },
    );
    let err = peek(&c, "a.json").unwrap_err();
    assert!(!format!("{err:?}").contains("secret raw text"), "{err:?}");
}

#[test]
fn static_provider_target_lookup_carries_box_env() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    c.peek_file(
        &BoxHandle::new("ssh/b1"),
        &BoxPath::under("/work", "a").unwrap(),
        Path::new("/host/cwd"),
    )
    .unwrap();
    let env = call_of(&p, Tool::Crabbox).env;
    assert!(env.contains(&("CRABBOX_STATIC_ID".to_owned(), "b1".to_owned())));
}

#[test]
fn target_lookup_runs_in_host_cwd() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    peek(&c, "a.json").unwrap();
    assert_eq!(
        call_of(&p, Tool::Crabbox).cwd.as_deref(),
        Some(Path::new("/host/cwd"))
    );
}

/// Delays `crabbox` calls by `latency`: past their deadline they time out.
struct Slow {
    inner: Arc<ScriptedProc>,
    latency: Duration,
}

impl Proc for Slow {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        if spec.tool == Tool::Crabbox && self.latency > spec.deadline.0 {
            return Err(ProcError::Timeout {
                tool: spec.tool,
                after: spec.deadline.0,
            });
        }
        self.inner.run(spec)
    }

    fn run_secret(&self, spec: &ProcSpec) -> Result<Zeroizing<Vec<u8>>, ProcError> {
        self.inner.run_secret(spec)
    }

    fn run_streamed(&self, spec: &ProcSpec, out: &Output) -> Result<(), ProcError> {
        self.inner.run_streamed(spec, out)
    }

    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError> {
        self.inner.spawn_detached(spec)
    }

    fn alive(&self, pid: u32) -> bool {
        self.inner.alive(pid)
    }
}

fn peek_slow(handle: &str) -> Result<Option<String>, DriverError> {
    let p = Arc::new(ScriptedProc::new());
    scripted(&p, Reply::Ok("x".to_owned()));
    let slow = Slow {
        inner: p,
        latency: Duration::from_secs(6),
    };
    Crabbox::fake(Arc::new(slow)).peek_file(
        &BoxHandle::new(handle),
        &BoxPath::under("/work", "a").unwrap(),
        Path::new("/host/cwd"),
    )
}

#[test]
fn external_provider_lookup_outlasts_a_six_second_ssh_lookup() {
    assert_eq!(peek_slow("external/box"), Ok(Some("x".to_owned())));
}

#[test]
fn built_in_provider_lookup_times_out_after_five_seconds() {
    assert_eq!(
        peek_slow("p/box"),
        Err(DriverError::Timeout(Duration::from_secs(5)))
    );
}

fn lookups(p: &ScriptedProc) -> usize {
    p.calls()
        .iter()
        .filter(|c| c.tool == Tool::Crabbox && c.args.first().map(String::as_str) == Some("ssh"))
        .count()
}

fn peek_of(c: &Crabbox, handle: &str) -> Result<Option<String>, DriverError> {
    c.peek_file(
        &BoxHandle::new(handle),
        &BoxPath::under("/work", "a").unwrap(),
        Path::new("/host/cwd"),
    )
}

#[test]
fn one_driver_looks_a_lease_up_once() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    peek(&c, "a").unwrap();
    peek(&c, "b").unwrap();
    let argv = c
        .connect_argv(&BoxHandle::new("p/box"), Path::new("/host/cwd"))
        .unwrap();
    assert_eq!(argv.first().map(String::as_str), Some("ssh"));
    assert_eq!(lookups(&p), 1);
}

#[test]
fn release_makes_the_next_lookup_resolve_again() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    peek(&c, "a").unwrap();
    c.release(&BoxHandle::new("p/box"), None).unwrap();
    peek(&c, "a").unwrap();
    assert_eq!(lookups(&p), 2);
}

#[test]
fn wake_makes_the_next_lookup_resolve_again() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    p.on(Tool::Crabbox, &["resume"], Reply::Ok(String::new()));
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(r#"[{"id":"p","sleep":"suspend","wake":true}]"#.to_owned()),
    );
    peek(&c, "a").unwrap();
    let _ = c.wake(&BoxHandle::new("p/box"), None);
    peek(&c, "a").unwrap();
    assert_eq!(lookups(&p), 2);
}

#[test]
fn another_lease_is_resolved_separately() {
    let (p, c) = fixture();
    scripted(&p, Reply::Ok("x".to_owned()));
    peek_of(&c, "p/one").unwrap();
    peek_of(&c, "p/two").unwrap();
    peek_of(&c, "p/one").unwrap();
    assert_eq!(lookups(&p), 2);
}

#[test]
fn a_failed_lookup_is_not_kept() {
    let (p, c) = fixture();
    p.once(
        Tool::Crabbox,
        &["ssh"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "boom".to_owned(),
        },
    );
    scripted(&p, Reply::Ok("x".to_owned()));
    assert!(peek(&c, "a").is_err());
    peek(&c, "a").unwrap();
    peek(&c, "a").unwrap();
    assert_eq!(lookups(&p), 2);
}
