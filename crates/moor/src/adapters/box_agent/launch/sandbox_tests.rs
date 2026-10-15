//! A launch with a Proxy runs the agent under the sandbox.

use super::*;
use crate::adapters::box_agent::nono_release::fake::{
    ready_release, release_of, tarball, FakeFetch, READY_BINARY,
};
use crate::adapters::box_agent::nono_release::{sha256_hex, NonoRelease};
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::engine::SessionRef;

const NOT_INSTALLED: i32 = 6;

fn uname_path() -> BoxPath {
    BoxPath::under(CRED, "uname").unwrap()
}

/// The outcome of a proxy launch on a Box whose checks return `codes`, whose `uname` says
/// `platform`, and whose downloads go through `release`.
fn launch_missing(
    codes: [i32; 2],
    platform: &str,
    release: NonoRelease,
) -> (
    Arc<FakeDriver>,
    Arc<FakeSurface>,
    Result<SessionRef, EngineError>,
) {
    let d = driver(
        FakeDriver::new()
            .exec_results([0, codes[0], codes[1]].map(|code| Ok(ExecOutcome::Exited { code }))),
    );
    d.set_peek_text("box-1", &uname_path(), platform);
    let s = surface(FakeSurface::new("/r"));
    let a = BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        Arc::new(release),
    );
    let out = a.launch(&req_with_proxy(&s));
    (d, s, out)
}

fn sandbox_message(out: Result<SessionRef, EngineError>) -> String {
    match out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    }
}

fn nono_put(d: &FakeDriver) -> Option<(BoxPath, Vec<u8>, u32)> {
    let put = (d.puts().into_iter()).find(|p| p.1.as_str().ends_with(".local/bin/nono"));
    put.map(|(_, path, bytes, mode, _)| (path, bytes, mode))
}

#[test]
fn a_proxy_launch_wraps_the_agent_in_the_sandbox() {
    let (_, s) = launch_with_proxy();
    let script = script_of(&pane_line(&s));
    let nono = script
        .find("'/home/box/.local/bin/nono' '-s' 'run' '--profile'")
        .expect(&script);
    let port = script.find("'--open-port' '4567'").expect(&script);
    let agent = script.rfind("'claude'").expect(&script);
    assert!(nono < port && port < agent, "{script}");
}

#[test]
fn the_check_runs_before_the_hooks_are_put() {
    let (d, _) = launch_with_proxy();
    let calls = d.calls();
    let exec = calls.iter().position(|c| c.starts_with("exec")).unwrap();
    let hooks = calls.iter().position(|c| c.starts_with("put")).unwrap();
    assert!(exec < hooks, "{calls:?}");
    let script = d.execs().remove(1).1.argv.pop().unwrap();
    assert!(script.contains("--version"), "{script}");
    assert!(script.contains(&sha256_hex(READY_BINARY)), "{script}");
    assert!(script.contains("setup --check-only"), "{script}");
}

#[test]
fn an_installed_tool_with_the_pinned_digest_is_not_uploaded() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"the binary"))));
    let release = release_of(&fetch, &tarball(b"the binary"));
    let (d, _, out) = launch_missing([0, 0], "Linux x86_64", release);
    out.unwrap();
    assert!(nono_put(&d).is_none());
    assert_eq!(d.execs().len(), 2);
}

#[test]
fn a_missing_or_replaced_tool_is_uploaded_executable_and_checked_again() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"the binary"))));
    let release = release_of(&fetch, &tarball(b"the binary"));
    let (d, _, out) = launch_missing([NOT_INSTALLED, 0], "Linux x86_64\n", release);
    out.unwrap();
    let (path, bytes, mode) = nono_put(&d).unwrap();
    assert_eq!(path.as_str(), "/home/box/.local/bin/nono");
    assert_eq!(bytes, b"the binary");
    assert_eq!(mode, 0o755);
    assert_eq!(fetch.urls().len(), 1);
    let execs = d.execs();
    let script = |i: usize| execs.get(i).unwrap().1.argv.last().unwrap().clone();
    assert!(script(0).contains("uname -s -m"), "{}", script(0));
    assert!(
        script(1).contains(&sha256_hex(b"the binary")),
        "{}",
        script(1)
    );
    assert_eq!(script(1), script(2));
}

#[test]
fn a_replaced_tool_whose_upload_fails_is_refused() {
    let d = driver(
        FakeDriver::new()
            .exec_results([0, NOT_INSTALLED].map(|code| Ok(ExecOutcome::Exited { code })))
            .fail_put(DriverError::NotFound),
    );
    let s = surface(FakeSurface::new("/r"));
    let out = BoxAgent::new(
        d,
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        ready_release(),
    )
    .launch(&req_with_proxy(&s));
    sandbox_message(out);
    assert!(s.opened().is_empty());
}

#[test]
fn an_unsupported_platform_fails_before_any_upload() {
    let fetch = Arc::new(FakeFetch::new(Ok(Vec::new())));
    let release = release_of(&fetch, b"x");
    let (d, s, out) = launch_missing([NOT_INSTALLED, 0], "Linux riscv64", release);
    let message = sandbox_message(out);
    assert!(message.contains("riscv64"), "{message}");
    assert!(fetch.urls().is_empty());
    assert!(nono_put(&d).is_none());
    assert!(s.opened().is_empty());
}

#[test]
fn a_download_that_fails_its_checksum_is_never_uploaded() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"tampered"))));
    let release = release_of(&fetch, &tarball(b"the binary"));
    let (d, s, out) = launch_missing([NOT_INSTALLED, 0], "Linux x86_64", release);
    let message = sandbox_message(out);
    assert!(message.contains("sha256 check"), "{message}");
    assert!(nono_put(&d).is_none());
    assert!(s.opened().is_empty());
}

#[test]
fn an_uploaded_tool_that_still_fails_the_check_fails_the_step() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"the binary"))));
    let release = release_of(&fetch, &tarball(b"the binary"));
    let (_, s, out) = launch_missing([NOT_INSTALLED, 5], "Linux x86_64", release);
    let message = sandbox_message(out);
    assert!(message.contains("Landlock"), "{message}");
    assert!(s.opened().is_empty());
}

#[test]
fn the_profile_lands_in_the_cred_dir_and_grants_the_temp_dir() {
    let (d, _) = launch_with_proxy();
    let put = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("nono-profile.json"));
    let (_, path, bytes, mode, _) = put.unwrap();
    assert_eq!(path.as_str(), format!("{CRED}/nono-profile.json"));
    assert_eq!(mode, 0o600);
    assert!(String::from_utf8_lossy(&bytes).contains(TMP));
}

#[test]
fn a_failed_check_fails_the_sandbox_step_and_opens_no_pane() {
    let (d, s) = (
        driver(FakeDriver::new().exec_results([0, 5].map(|code| Ok(ExecOutcome::Exited { code })))),
        surface(FakeSurface::new("/r")),
    );
    let message = sandbox_message(agent(&d, &s).launch(&req_with_proxy(&s)));
    assert!(message.contains("Landlock"), "{message}");
    assert!(s.opened().is_empty());
    assert_eq!(d.execs().pop().unwrap().1.argv, ["rm", "-rf", CRED, TMP]);
}

#[test]
fn a_launch_without_a_proxy_installs_nothing_and_stays_unsandboxed() {
    let (d, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    assert!(d.execs().is_empty());
    assert!(!pane_line(&s).contains("nono"));
}

#[test]
fn a_failed_proxy_stage_removes_the_cred_dir() {
    let (d, s) = (
        driver(FakeDriver::new().fail_put(DriverError::NotFound)),
        surface(FakeSurface::new("/r")),
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
fn the_proxy_env_is_exported_and_its_names_are_allowed_in_the_profile() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new("/r")));
    let addr = ProxyAddr {
        box_env: vec![("APP_MODE".to_owned(), "dev".to_owned())],
        ..proxy_addr()
    };
    let req = LaunchReq {
        proxy: Some(Arc::new(addr)),
        ..req_with_proxy(&s)
    };
    agent(&d, &s).launch(&req).unwrap();
    let profile = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .unwrap();
    assert!(String::from_utf8_lossy(&profile.2).contains("\"APP_MODE\""));
    let script = script_of(&pane_line(&s));
    assert!(!script.contains("export APP_MODE="), "{script}");
}

#[test]
fn repo_env_reaches_the_agent_only_through_the_profile() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new("/r")));
    let value = "it's\n$(id) `id`";
    let addr = ProxyAddr {
        box_env: vec![
            ("APP_NOTE".to_owned(), value.to_owned()),
            ("NONO_ALLOW".to_owned(), "/".to_owned()),
            ("XDG_CONFIG_HOME".to_owned(), "/tmp/x".to_owned()),
        ],
        ..proxy_addr()
    };
    let req = LaunchReq {
        proxy: Some(Arc::new(addr)),
        ..req_with_proxy(&s)
    };
    agent(&d, &s).launch(&req).unwrap();
    let script = script_of(&pane_line(&s));
    for name in ["APP_NOTE", "NONO_ALLOW"] {
        assert!(!script.contains(&format!("export {name}=")), "{script}");
    }
    assert!(!script.contains("/tmp/x"), "{script}");
    assert!(!script.contains("$(id)"), "{script}");
    assert!(script.contains("unset \"$moor_v\""), "{script}");
    assert!(
        script.contains("export XDG_CONFIG_HOME=\"$HOME/.config\""),
        "{script}"
    );
    let harden = script.find("XDG_CONFIG_HOME").expect(&script);
    assert!(
        harden < script.find("'--profile'").expect(&script),
        "{script}"
    );
    let profile = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&profile.2).unwrap();
    assert_eq!(
        doc.pointer("/environment/set_vars/APP_NOTE"),
        Some(&serde_json::Value::from(value))
    );
}
