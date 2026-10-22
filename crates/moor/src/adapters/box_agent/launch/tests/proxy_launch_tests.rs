//! A launch with a Proxy exports its environment.

use super::*;

#[test]
fn a_launch_without_a_proxy_installs_nothing() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let execs = d.execs();
    assert_eq!(execs.len(), 1);
    let script = execs.iter().find_map(|e| e.1.argv.last()).unwrap();
    assert!(script.contains("command -v"), "{script}");
    assert!(d.puts().iter().all(|p| !p.1.as_str().contains("proxy-")));
}

#[test]
fn a_failed_put_removes_the_cred_dir() {
    let (d, s) = (
        driver(FakeDriver::new().fail_put(DriverError::NotFound)),
        surface(FakeSurface::new()),
    );
    let out = agent(&d, &s).launch(&req_with_proxy(&s));
    assert!(matches!(out, Err(EngineError::Failed { .. })));
    assert!(s.opened().is_empty());
    assert_eq!(d.execs().pop().unwrap().1.argv, ["rm", "-rf", CRED, TMP]);
}

#[test]
fn the_agent_gets_a_private_temp_dir_that_is_removed_at_exit() {
    let b = Scratch::new();
    let tmp = b.path("home/.moor-tmp-t1-x");
    b.stub(
        "claude",
        "touch \"$MARKER\"; echo \"$TMPDIR $CLAUDE_CODE_TMPDIR\" > \"$MARKER.tmp\"; \
         ls -ld \"$TMPDIR\" > \"$MARKER.ls\"",
    );
    assert!(b.spawn(&b.script("true")).wait().unwrap().success());
    let seen = std::fs::read_to_string(b.path("started.tmp")).unwrap();
    assert_eq!(seen.trim(), format!("{0} {0}", tmp.display()));
    let ls = std::fs::read_to_string(b.path("started.ls")).unwrap();
    assert!(ls.starts_with("drwx------"), "{ls}");
    assert!(!tmp.exists());
}

#[test]
fn a_failed_script_step_removes_the_temp_dir() {
    let b = Scratch::new();
    assert!(!b.spawn(&b.script("false")).wait().unwrap().success());
    assert!(!b.path("home/.moor-tmp-t1-x").exists());
}

#[test]
fn a_repo_env_value_is_quoted_so_it_cannot_run_in_the_script() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    let addr = ProxyAddr {
        box_env: vec![("APP_NOTE".to_owned(), "it's\n$(id) `id`".to_owned())],
        ..proxy_addr()
    };
    let req = LaunchReq {
        proxy: Some(Arc::new(addr)),
        ..req_with_proxy(&s)
    };
    agent(&d, &s).launch(&req).unwrap();
    let script = script_of(&pane_line(&s));
    let set = script
        .find("export APP_NOTE='it'\\''s\n$(id) `id`'")
        .expect(&script);
    assert!(set < script.rfind("'claude'").expect(&script), "{script}");
}

#[test]
fn a_repo_env_value_cannot_replace_the_root_signal() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    d.set_peek_text("box-1", &environ_path(), "HOME=/home/box\0USER=root\0");
    let addr = ProxyAddr {
        box_env: vec![("IS_SANDBOX".to_owned(), "0".to_owned())],
        ..proxy_addr()
    };
    let req = LaunchReq {
        proxy: Some(Arc::new(addr)),
        ..req_with_proxy(&s)
    };
    agent(&d, &s).launch(&req).unwrap();
    let script = script_of(&pane_line(&s));
    let last = script
        .lines()
        .rfind(|l| l.contains("IS_SANDBOX"))
        .expect(&script);
    assert_eq!(last.trim(), "export IS_SANDBOX=1", "{script}");
    let root = script.rfind("export IS_SANDBOX=1").expect(&script);
    assert!(root < script.rfind("'claude'").expect(&script), "{script}");
}

#[test]
fn a_ca_that_cannot_be_written_stops_the_launch_before_the_agent_with_the_ca_message() {
    let b = Scratch::new();
    b.stub("sudo", "exit 1");
    std::fs::write(b.path("home/.moor-proxy"), "a file, not a directory").unwrap();
    let script = format!(
        "exec 2>\"$MARKER.err\"\n{}",
        b.script_with("true", &b.proxy_lines())
    );
    assert!(!b.spawn(&script).wait().unwrap().success());
    assert!(!b.agent_started());
    assert!(b.cred_gone());
    let err = std::fs::read_to_string(b.path("started.err")).unwrap();
    let ca = b.path("home/.moor-proxy/ca.pem");
    assert!(
        err.contains(&format!(
            "moor: the Proxy CA could not be written to {}",
            ca.display()
        )),
        "{err}"
    );
}
