use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

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
