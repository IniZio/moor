use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::agent_adapter::{AgentAdapter, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchPrompt, LaunchReq, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::proxy_env;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::box_name;
use crate::testing::driver::FakeDriver;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";
const CRED: &str = "/home/box/.moor-cred-1-abc123";
const TMP: &str = "/home/box/.moor-tmp-1-abc123";

fn environ_path() -> BoxPath {
    BoxPath::restore("/proc/self/environ").unwrap()
}

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(d.peek_text("box-1", &environ_path(), ENVIRON));
    d.acquire(&AcquireReq {
        pool_machine: None,
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

fn req() -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(1),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        box_name: box_name("t1"),
        branch: "moor/t1".to_owned(),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        disk_note: None,
        shape: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        strip_env: Vec::new(),
        prompt: LaunchPrompt::None,
    }
}

fn agent(d: &Arc<FakeDriver>) -> BoxAgent {
    BoxAgent::new(d.clone(), Arc::new(Claude))
}

fn launch(d: FakeDriver) -> (Arc<FakeDriver>, Result<SessionRef, EngineError>) {
    let d = driver(d);
    let out = agent(&d).launch(&req());
    (d, out)
}

/// The command that starts the agent's terminal on the Box.
fn start_of(d: &FakeDriver) -> String {
    let starts: Vec<String> = d
        .execs()
        .into_iter()
        .filter_map(|(_, spec, _)| spec.argv.last().cloned())
        .filter(|script| script.contains("herdr-core run"))
        .collect();
    assert_eq!(starts.len(), 1, "{starts:?}");
    starts.into_iter().next().unwrap()
}

#[test]
fn delivers_the_brief_under_the_box_home() {
    let (d, out) = launch(FakeDriver::new());
    let session = out.unwrap();
    assert_eq!(session.id, "");
    assert_eq!(session.handle, BoxHandle::new("box-1"));
    assert_eq!(session.workdir, BoxPath::under("/work", "box-1").unwrap());
    let (handle, path, bytes, mode) = d.puts().into_iter().next().unwrap();
    assert_eq!(handle, BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), format!("{CRED}/brief.md"));
    assert!(String::from_utf8_lossy(&bytes).starts_with("# Box t1\n"));
    assert_eq!(mode, 0o600);
}

#[test]
fn the_delivered_brief_carries_the_repo_notes_and_the_shape() {
    let d = driver(FakeDriver::new());
    let mut r = req();
    r.repo_notes = Some("run make emu".to_owned());
    r.shape = Some(moor_core::shape::Shape {
        cpus: 2,
        mem_mib: 4096,
    });
    agent(&d).launch(&r).unwrap();
    let (_, _, bytes, _) = d.puts().into_iter().next().unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("## This project's Box"), "{text}");
    assert!(text.contains("run make emu"), "{text}");
    assert!(text.contains("2 vCPUs and 4.0 GiB"), "{text}");
}

#[test]
fn delivers_the_hook_writer_files() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let paths: Vec<_> = d.puts().iter().map(|p| p.1.as_str().to_owned()).collect();
    assert_eq!(
        paths,
        [
            format!("{CRED}/brief.md"),
            "/work/box-1/.moor/hook.sh".to_owned(),
            "/work/box-1/.moor/hooks.json".to_owned(),
            format!("{CRED}/agent.sh"),
        ]
    );
}

#[test]
fn a_resume_runs_the_agent_with_the_stored_session_instead_of_the_prompt() {
    let d = driver(FakeDriver::new());
    let r = LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        resume: Some("sess-9".to_owned()),
        ..req()
    };
    agent(&d).launch(&r).unwrap();
    let script = script_of(&d);
    assert!(
        script.contains("'claude' '--resume' 'sess-9' '--append-system-prompt-file' '/work/box-1/.moor/brief.md' '--model' 'haiku'"),
        "{script}"
    );
    assert!(!script.contains("Read and follow"), "{script}");
}

#[test]
fn the_script_cleans_up_installs_the_brief_and_starts_the_agent() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let script = script_of(&d);
    for want in [
        &format!("trap 'rm -rf '\\''{CRED}'\\'' '\\''{TMP}'\\''' EXIT"),
        &format!("mkdir -p -m 700 '{TMP}' && mkdir -p '/work/box-1/.moor'"),
        &format!("export TMPDIR='{TMP}' CLAUDE_CODE_TMPDIR='{TMP}'"),
        &format!("install -m 600 '{CRED}/brief.md' '/work/box-1/.moor/brief.md'"),
        "settings.json",
        ".claude.json",
        "cd '/work/box-1' && exec 'claude' '--append-system-prompt-file' '/work/box-1/.moor/brief.md' '--model' 'haiku'",
    ] {
        assert!(script.contains(want), "{want}: {script}");
    }
    assert!(!script.contains("bypassPermissions"));
    assert!(!script.contains("ANTHROPIC"));
}

/// The agent script the launch put on the Box.
fn script_of(d: &FakeDriver) -> String {
    let put = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("/agent.sh"));
    String::from_utf8(put.expect("the launch puts the agent script").2).unwrap()
}

#[test]
fn herdr_core_holds_the_agent_running_the_script_file_under_sh() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let start = start_of(&d);
    let want = format!(
        "herdr-core kill moor-agent >/dev/null 2>&1; herdr-core rm moor-agent >/dev/null 2>&1; \
         herdr-core run --bg --no-parent --name moor-agent sh '{CRED}/agent.sh' >/dev/null || exit 1;"
    );
    assert!(start.contains(&want), "{start}");
    assert!(!start.contains("--rm"), "{start}");
}

#[test]
fn the_start_checks_for_herdr_core_and_refuses_with_its_exit_status() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let start = start_of(&d);
    assert!(start.contains("command -v herdr-core"), "{start}");
    assert!(start.contains("exit 127"), "{start}");
}

#[test]
fn a_box_without_herdr_core_fails_the_start_step_and_removes_the_cred_dir() {
    let (d, out) = launch(
        FakeDriver::new().exec_results([0, 127].map(|code| Ok(ExecOutcome::Exited { code }))),
    );
    let message = match out {
        Err(EngineError::Failed {
            step: "start",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    };
    assert!(message.contains("herdr-core is not installed"), "{message}");
    assert_eq!(d.execs().pop().unwrap().1.argv, ["rm", "-rf", CRED, TMP]);
}

#[test]
fn the_agent_is_reached_through_herdr_core() {
    let d = driver(FakeDriver::new());
    let kit = agent(&d);
    let sref = kit.launch(&req()).unwrap();
    let before = d.execs().len();
    kit.probe(&sref, Deadline(Duration::from_secs(1)));
    kit.stop(&sref).unwrap();
    let mut cmds: Vec<String> = d
        .execs()
        .iter()
        .skip(before)
        .map(|(_, spec, _)| spec.argv.join(" "))
        .collect();
    cmds.extend(kit.relay_argv(&["ssh".into()]).unwrap());
    let cmds = cmds.join("\n");
    for want in ["herdr-core get", "herdr-core kill", "herdr-core attach"] {
        assert!(cmds.contains(want), "{want}: {cmds}");
    }
}

#[test]
fn the_install_puts_herdr_core_on_the_box_before_the_agent() {
    let d = driver(FakeDriver::new());
    let claude = Claude.install().unwrap();
    let hc = agent(&d).agent_install().unwrap();
    let (own, rest) = hc.script.split_once(&claude.script).unwrap();
    assert!(own.contains("releases.herdr.dev/core/0.1.16/"), "{own}");
    assert!(rest.is_empty(), "{rest}");
    assert_eq!(hc.dirs.last(), claude.dirs.last());
    assert!(hc.dirs.len() > claude.dirs.len());
}

#[test]
fn the_script_is_valid_shell() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let mut sh = moor_testkit::ChildGuard::new(
        Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    sh.stdin
        .take()
        .unwrap()
        .write_all(script_of(&d).as_bytes())
        .unwrap();
    assert!(sh.wait().unwrap().success());
}

#[test]
fn a_failed_connect_lookup_delivers_nothing() {
    let (d, out) = launch(FakeDriver::new().fail_connect(DriverError::NotFound));
    assert!(matches!(
        out,
        Err(EngineError::Failed {
            step: "connect",
            ..
        })
    ));
    assert!(d.puts().is_empty());
    assert!(d.execs().is_empty());
}

#[test]
fn a_failed_brief_write_removes_the_cred_dir() {
    let (d, out) = launch(
        FakeDriver::new().fail_put(DriverError::Timeout(std::time::Duration::from_secs(30))),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed { step: "brief", .. })
    ));
    let execs = d.execs();
    assert_eq!(execs.len(), 2);
    assert_eq!(
        execs.into_iter().last().unwrap().1.argv,
        ["rm", "-rf", CRED, TMP]
    );
}

#[test]
fn a_failed_cleanup_is_named_in_the_error() {
    let (_, out) = launch(
        FakeDriver::new()
            .fail_put(DriverError::NotFound)
            .exec_results([0, 1].map(|code| Ok(ExecOutcome::Exited { code }))),
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
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: box_name("t1"),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    let r = req();
    let out = agent(&d).launch(&r);
    assert!(matches!(out, Err(EngineError::Failed { step: "home", .. })));
    assert!(d.puts().is_empty());
}

fn proxy_addr() -> ProxyAddr {
    ProxyAddr {
        box_port: 4567,
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        box_env: Vec::new(),
        egress_port: 4568,
        hosts: Vec::new(),
    }
}

fn req_with_proxy() -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        proxy: Some(Arc::new(proxy_addr())),
        ..req()
    }
}

fn launch_with_proxy() -> Arc<FakeDriver> {
    let d = driver(FakeDriver::new());
    agent(&d).launch(&req_with_proxy()).unwrap();
    d
}

#[test]
fn a_proxy_launch_stages_only_the_brief_and_the_agent_script() {
    let d = launch_with_proxy();
    let staged: Vec<_> = d
        .puts()
        .into_iter()
        .filter(|p| p.1.as_str().starts_with(&format!("{CRED}/")))
        .map(|p| (p.1.as_str().to_owned(), p.3))
        .collect();
    assert_eq!(
        staged,
        [
            (format!("{CRED}/brief.md"), 0o600),
            (format!("{CRED}/agent.sh"), 0o600)
        ]
    );
}

#[test]
fn a_proxy_launch_removes_the_cred_dir_after_the_brief_and_exports_the_proxy_env() {
    let d = launch_with_proxy();
    let script = script_of(&d);
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
mod profile_tests;
mod proxy_launch_tests;
mod scratch_run;
mod secret_tests;

#[test]
fn the_script_ends_with_the_agent_and_prints_no_status_of_its_own() {
    let (d, out) = launch(FakeDriver::new());
    out.unwrap();
    let script = script_of(&d);
    let last = script.lines().last().unwrap();
    assert!(
        last.starts_with("cd '/work/box-1' && exec 'claude' "),
        "{script}"
    );
    assert!(!script.contains("MOOR_AGENT_TTY"), "{script}");
}
