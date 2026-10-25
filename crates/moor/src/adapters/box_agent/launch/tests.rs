use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchPrompt, LaunchReq};
use moor_core::seam::surface::{Surface, SurfaceError};
use moor_core::{BoxHandle, BoxId, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::fake::ready_release;
use crate::adapters::box_agent::proxy_env;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::box_name;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";
const CRED: &str = "/home/box/.moor-cred-1-abc123";
const TMP: &str = "/home/box/.moor-tmp-1-abc123";

fn environ_path() -> BoxPath {
    BoxPath::restore("/proc/self/environ").unwrap()
}

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(
        d.peek_text("box-1", &environ_path(), ENVIRON)
            .run_stdout("Linux x86_64"),
    );
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: box_name("t1"),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    d
}

fn surface(s: FakeSurface) -> Arc<FakeSurface> {
    Arc::new(s)
}

fn req(s: &FakeSurface) -> LaunchReq {
    LaunchReq {
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(1),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        workspace: s.ensure_workspace("moor:t1").unwrap(),
        box_name: box_name("t1"),
        branch: "moor/t1".to_owned(),
        goal: Some("fix x".to_owned()),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        shape: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        strip_env: Vec::new(),
        prompt: LaunchPrompt::Brief,
    }
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        ready_release(),
    )
}

fn launch(
    d: FakeDriver,
    s: FakeSurface,
) -> (
    Arc<FakeDriver>,
    Arc<FakeSurface>,
    Result<String, EngineError>,
) {
    let (d, s) = (driver(d), surface(s));
    let r = req(&s);
    let out = agent(&d, &s)
        .launch(&r)
        .map(|s| s.pane.unwrap().as_str().to_owned());
    (d, s, out)
}

fn pane_line(s: &FakeSurface) -> String {
    s.opened().remove(0).argv.pop().unwrap()
}

#[test]
fn delivers_the_brief_under_the_box_home() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new());
    assert_eq!(out.unwrap(), "p-2");
    let (handle, path, bytes, mode) = d.puts().into_iter().next().unwrap();
    assert_eq!(handle, BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), format!("{CRED}/brief.md"));
    assert!(String::from_utf8_lossy(&bytes).contains("Goal: fix x"));
    assert_eq!(mode, 0o600);
}

#[test]
fn the_delivered_brief_carries_the_repo_notes_and_the_shape() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    let mut r = req(&s);
    r.repo_notes = Some("run make emu".to_owned());
    r.shape = Some(moor_core::shape::Shape {
        cpus: 2,
        mem_mib: 4096,
    });
    agent(&d, &s).launch(&r).unwrap();
    let (_, _, bytes, _) = d.puts().into_iter().next().unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("## This project's Box"), "{text}");
    assert!(text.contains("run make emu"), "{text}");
    assert!(text.contains("2 vCPUs and 4.0 GiB"), "{text}");
}

#[test]
fn delivers_the_hook_writer_files() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let paths: Vec<_> = d.puts().iter().map(|p| p.1.as_str().to_owned()).collect();
    assert_eq!(
        paths,
        [
            format!("{CRED}/brief.md"),
            "/work/box-1/.moor/hook.sh".to_owned(),
            "/work/box-1/.moor/hooks.json".to_owned(),
        ]
    );
}

#[test]
fn opens_an_unfocused_agent_tab() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let cmd = s.opened().remove(0);
    assert_eq!(cmd.label, "agent:t1");
    assert!(cmd.env.is_empty());
    assert!(!s.calls().iter().any(|c| c.starts_with("focus")));
}

#[test]
fn the_pane_line_runs_the_script_without_exec_and_reports_the_exit() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let line = pane_line(&s);
    assert!(line.starts_with("'ssh' '-t' 'box-1' '"), "{line}");
    assert!(line.ends_with(r#"echo "moor: agent exited $c""#), "{line}");
    assert!(!line.contains("exec "), "{line}");
}

#[test]
fn a_resume_runs_the_agent_with_the_stored_session_instead_of_the_prompt() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    let r = LaunchReq {
        mcp: std::collections::BTreeMap::new(),
        resume: Some("sess-9".to_owned()),
        ..req(&s)
    };
    agent(&d, &s).launch(&r).unwrap();
    let script = script_of(&pane_line(&s));
    assert!(
        script.contains("'claude' '--resume' 'sess-9' '--model' 'haiku'"),
        "{script}"
    );
    assert!(!script.contains("Read and follow"), "{script}");
}

#[test]
fn the_script_cleans_up_installs_the_brief_and_starts_the_agent() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let line = pane_line(&s);
    let script = script_of(&line);
    for want in [
        &format!("trap 'rm -rf '\\''{CRED}'\\'' '\\''{TMP}'\\''' EXIT"),
        &format!("mkdir -p -m 700 '{TMP}' && mkdir -p '/work/box-1/.moor'"),
        &format!("export TMPDIR='{TMP}' CLAUDE_CODE_TMPDIR='{TMP}'"),
        &format!("install -m 600 '{CRED}/brief.md' '/work/box-1/.moor/brief.md'"),
        r#""state":"starting","event":"Launch""#,
        "> '/work/box-1/.moor/agent-state.json'",
        "settings.json",
        ".claude.json",
        "cd '/work/box-1' && 'claude' '--model' 'haiku'",
    ] {
        assert!(script.contains(want), "{want}: {script}");
    }
    assert!(!line.contains("bypassPermissions"));
    assert!(!line.contains("ANTHROPIC"));
}

fn unquote(q: &str) -> String {
    let inner = q.strip_prefix('\'').and_then(|q| q.strip_suffix('\''));
    inner.unwrap().replace("'\\''", "'")
}

/// The command the Box is asked to run.
fn remote_of(line: &str) -> String {
    let rest = line.strip_prefix("'ssh' '-t' 'box-1' ").unwrap();
    let quoted = rest
        .strip_suffix(r#"; c=$?; printf '\r\033[K'; echo "moor: agent exited $c""#)
        .unwrap();
    unquote(quoted)
}

fn script_of(line: &str) -> String {
    unquote(remote_of(line).strip_prefix("sh -c ").unwrap())
}

#[test]
fn the_box_runs_the_script_under_sh_not_the_login_shell() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    assert!(remote_of(&pane_line(&s)).starts_with("sh -c '"));
}

#[test]
fn the_script_is_valid_shell() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new());
    out.unwrap();
    let mut sh = Command::new("sh")
        .arg("-n")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin
        .take()
        .unwrap()
        .write_all(script_of(&pane_line(&s)).as_bytes())
        .unwrap();
    assert!(sh.wait().unwrap().success());
}

#[test]
fn a_failed_connect_lookup_opens_no_pane_and_delivers_nothing() {
    let (d, s, out) = launch(
        FakeDriver::new().fail_connect(DriverError::NotFound),
        FakeSurface::new(),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed {
            step: "connect",
            ..
        })
    ));
    assert!(s.opened().is_empty());
    assert!(d.puts().is_empty());
    assert!(d.execs().is_empty());
}

#[test]
fn a_failed_brief_write_removes_the_cred_dir() {
    let (d, s, out) = launch(
        FakeDriver::new().fail_put(DriverError::Timeout(std::time::Duration::from_secs(30))),
        FakeSurface::new(),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed { step: "brief", .. })
    ));
    assert!(s.opened().is_empty());
    let execs = d.execs();
    assert_eq!(execs.len(), 2);
    assert_eq!(
        execs.into_iter().last().unwrap().1.argv,
        ["rm", "-rf", CRED, TMP]
    );
}

#[test]
fn a_failed_pane_open_removes_the_cred_dir() {
    let (d, _, out) = launch(
        FakeDriver::new(),
        FakeSurface::new().fail_open_pane(SurfaceError::Timeout),
    );
    assert!(matches!(out, Err(EngineError::Failed { step: "pane", .. })));
    assert_eq!(d.execs().pop().unwrap().1.argv, ["rm", "-rf", CRED, TMP]);
}

#[test]
fn a_failed_cleanup_is_named_in_the_error() {
    let (_, _, out) = launch(
        FakeDriver::new()
            .fail_put(DriverError::NotFound)
            .exec_results([0, 1].map(|code| Ok(ExecOutcome::Exited { code }))),
        FakeSurface::new(),
    );
    let message = format!("{out:?}");
    assert!(
        message.contains(&format!("could not remove {CRED}")),
        "{message}"
    );
}

#[test]
fn a_box_without_a_home_fails_before_delivering() {
    let d = Arc::new(FakeDriver::new().peek_text("box-1", &environ_path(), "PATH=/usr/bin\0"));
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: box_name("t1"),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    let s = surface(FakeSurface::new());
    let r = req(&s);
    let out = agent(&d, &s).launch(&r);
    assert!(matches!(out, Err(EngineError::Failed { step: "home", .. })));
    assert!(d.puts().is_empty());
    assert!(s.opened().is_empty());
}

fn proxy_addr() -> ProxyAddr {
    ProxyAddr {
        box_port: 4567,
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        box_env: Vec::new(),
        forge: false,
        egress_port: 4568,
        hosts: Vec::new(),
    }
}

fn req_with_proxy(s: &FakeSurface) -> LaunchReq {
    LaunchReq {
        mcp: std::collections::BTreeMap::new(),
        proxy: Some(Arc::new(proxy_addr())),
        ..req(s)
    }
}

fn launch_with_proxy() -> (Arc<FakeDriver>, Arc<FakeSurface>) {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    agent(&d, &s).launch(&req_with_proxy(&s)).unwrap();
    (d, s)
}

#[test]
fn a_proxy_launch_stages_only_the_brief() {
    let (d, _) = launch_with_proxy();
    let staged: Vec<_> = d
        .puts()
        .into_iter()
        .filter(|p| p.1.as_str().starts_with(&format!("{CRED}/")))
        .map(|p| (p.1.as_str().to_owned(), p.3))
        .collect();
    assert_eq!(staged, [(format!("{CRED}/brief.md"), 0o600)]);
}

#[test]
fn a_proxy_launch_removes_the_cred_dir_after_the_brief_and_exports_the_proxy_env() {
    let (_, s) = launch_with_proxy();
    let script = script_of(&pane_line(&s));
    assert!(script.contains(&format!("\nrm -rf '{CRED}'\n")), "{script}");
    assert!(!script.contains("GIT_CONFIG"), "{script}");
    for want in [
        "HTTPS_PROXY",
        "SSL_CERT_FILE",
        "unset GH_TOKEN GITHUB_TOKEN",
    ] {
        assert!(script.contains(want), "{want}: {script}");
    }
}

#[test]
fn the_agent_gets_the_proxy_env_and_the_ca_after_the_cred_dir_is_gone() {
    let b = Scratch::new();
    let cred = b.path("home/.moor-cred-t1-x");
    b.stub(
        "claude",
        &format!(
            "touch \"$MARKER\"; env > \"$MARKER.env\"; \
             if [ -d '{c}' ]; then touch \"$MARKER.dir\"; fi",
            c = cred.display()
        ),
    );
    let lines: Vec<_> = b
        .proxy_lines()
        .into_iter()
        .filter(|l| !l.contains("sudo"))
        .collect();
    let script = b.script_with("true", &lines);
    assert!(b.spawn(&script).wait().unwrap().success());
    let env = std::fs::read_to_string(b.path("started.env")).unwrap();
    let line = |name: &str| env.lines().find(|l| l.starts_with(name)).unwrap_or("");
    assert_eq!(
        line("HTTPS_PROXY="),
        format!(
            "HTTPS_PROXY=http://127.0.0.1:{}",
            b.proxy_addr().egress_port
        ),
        "{env}"
    );
    assert!(!env.contains("GH_TOKEN"), "{env}");
    let ca = b.path("home/.moor-proxy/ca.pem");
    assert_eq!(
        line("NODE_EXTRA_CA_CERTS="),
        format!("NODE_EXTRA_CA_CERTS={}", ca.display()),
        "{env}"
    );
    assert_eq!(std::fs::read_to_string(&ca).unwrap(), proxy_addr().ca_pem);
    assert!(!b.path("started.dir").exists());
    assert!(b.cred_gone());
}

use scratch_run::Scratch;

mod batch;
mod env_tests;
mod proxy_launch_tests;
mod scratch_run;
mod secret_tests;
