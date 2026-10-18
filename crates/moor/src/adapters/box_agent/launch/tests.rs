use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq};
use moor_core::seam::surface::{Surface, SurfaceError};
use moor_core::{BoxHandle, BoxPath, ProviderId, Secret, TaskId, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::fake::ready_release;
use crate::adapters::box_agent::proxy_env;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";
const CRED: &str = "/home/box/.moor-cred-t1-abc123";
const TMP: &str = "/home/box/.moor-tmp-t1-abc123";

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
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

fn surface(s: FakeSurface) -> Arc<FakeSurface> {
    Arc::new(s)
}

fn req(s: &FakeSurface) -> LaunchReq {
    let wt = s.create_worktree(&PathBuf::from("/r"), &TaskName::parse("t1").unwrap(), None);
    let wt = wt.unwrap();
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: wt.path.clone(),
        workspace: s.ensure_workspace(&wt, "moor:t1").unwrap(),
        task: TaskName::parse("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        goal: Some("fix x".to_owned()),
        resume: None,
        issue: None,
        setup_exit: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        origin: None,
        strip_env: Vec::new(),
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
fn delivers_the_brief_under_the_box_home_with_the_worktree_as_host_cwd() {
    let (d, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    assert_eq!(out.unwrap(), "p-2");
    let (handle, path, bytes, mode, cwd) = d.puts().into_iter().next().unwrap();
    assert_eq!(handle, BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), format!("{CRED}/brief.md"));
    assert!(String::from_utf8_lossy(&bytes).contains("Goal: fix x"));
    assert_eq!(mode, 0o600);
    assert_eq!(Some(cwd), s.opened().remove(0).cwd);
}

#[test]
fn delivers_the_hook_writer_files() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
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
fn opens_an_unfocused_agent_tab_in_the_host_worktree() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let cmd = s.opened().remove(0);
    assert_eq!(cmd.label, "agent:t1");
    assert_eq!(cmd.cwd, Some(PathBuf::from("/r/wt/t1")));
    assert!(cmd.env.is_empty());
    assert!(!s.calls().iter().any(|c| c.starts_with("focus")));
}

#[test]
fn the_pane_line_runs_the_script_without_exec_and_reports_the_exit() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let line = pane_line(&s);
    assert!(line.starts_with("'ssh' '-t' 'box-1' '"), "{line}");
    assert!(line.ends_with(r#"echo "moor: agent exited $c""#), "{line}");
    assert!(!line.contains("exec "), "{line}");
}

#[test]
fn a_resume_runs_the_agent_with_the_stored_session_instead_of_the_prompt() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new("/r")));
    let r = LaunchReq {
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
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
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
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    assert!(remote_of(&pane_line(&s)).starts_with("sh -c '"));
}

#[test]
fn the_script_is_valid_shell() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
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
        FakeSurface::new("/r"),
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
        FakeSurface::new("/r"),
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
        FakeSurface::new("/r").fail_open_pane(SurfaceError::Timeout),
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
        FakeSurface::new("/r"),
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
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    let s = surface(FakeSurface::new("/r"));
    let r = req(&s);
    let out = agent(&d, &s).launch(&r);
    assert!(matches!(out, Err(EngineError::Failed { step: "home", .. })));
    assert!(d.puts().is_empty());
    assert!(s.opened().is_empty());
}

/// A scratch Box: home, workdir, a credential dir holding a brief, and a stub agent.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let s = Self { dir };
        for d in ["home", "work", "bin", "home/.moor-cred-t1-x"] {
            std::fs::create_dir_all(s.path(d)).unwrap();
        }
        std::fs::write(s.path("home/.moor-cred-t1-x/brief.md"), "brief").unwrap();
        s.stub("claude", "touch \"$MARKER\"");
        s
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn stub(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = self.path(&format!("bin/{name}"));
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn script(&self, hook_step: &str) -> String {
        self.script_with(hook_step, &[])
    }

    fn proxy_ctx(&self) -> LaunchCtx {
        let root = self.dir.path().to_str().unwrap();
        LaunchCtx {
            model: None,
            workdir: BoxPath::under(root, "work").unwrap(),
            proxy: Some(proxy_addr()),
            home: BoxPath::under(root, "home").unwrap(),
        }
    }

    fn proxy_lines(&self) -> Vec<String> {
        let root = self.dir.path().to_str().unwrap();
        let cred = BoxPath::under(root, "home/.moor-cred-t1-x").unwrap();
        let env = Claude.env(&self.proxy_ctx());
        let origin = Some("git@github.com:o/n.git");
        proxy_env::script_lines(&cred, &proxy_addr(), &env, origin).unwrap()
    }

    fn script_with(&self, hook_step: &str, proxy: &[String]) -> String {
        let root = self.dir.path().to_str().unwrap();
        let ctx = LaunchCtx {
            model: None,
            workdir: BoxPath::under(root, "work").unwrap(),
            proxy: None,
            home: BoxPath::under(root, "home").unwrap(),
        };
        let setup = |step: &str| BoxSetup {
            files: Vec::new(),
            step: step.to_owned(),
        };
        let cred = BoxPath::under(root, "home/.moor-cred-t1-x").unwrap();
        let tmp = BoxPath::under(root, "home/.moor-tmp-t1-x").unwrap();
        super::script(
            (&cred, &tmp),
            &ctx,
            &setup(hook_step),
            &setup("true"),
            &[],
            (proxy, &[]),
        )
        .unwrap()
    }

    fn spawn(&self, script: &str) -> std::process::Child {
        use std::os::unix::process::CommandExt;
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new("sh")
            .args(["-c", script])
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("MARKER", self.path("started"))
            .env("GH_TOKEN", "parent-token")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn cred_gone(&self) -> bool {
        !self.path("home/.moor-cred-t1-x").exists()
    }

    fn agent_started(&self) -> bool {
        self.path("started").exists()
    }
}

#[test]
fn a_clean_run_starts_the_agent_and_removes_the_cred_dir() {
    let b = Scratch::new();
    assert!(b.spawn(&b.script("true")).wait().unwrap().success());
    assert!(b.agent_started());
    assert!(b.cred_gone());
    assert_eq!(
        std::fs::read_to_string(b.path("work/.moor/brief.md")).unwrap(),
        "brief"
    );
}

#[test]
fn the_cred_dir_is_gone_before_the_agent_starts() {
    let b = Scratch::new();
    let cred = b.path("home/.moor-cred-t1-x");
    b.stub(
        "claude",
        &format!(
            "touch \"$MARKER\"; if [ -e '{}' ]; then touch \"$MARKER.cred\"; fi",
            cred.display()
        ),
    );
    assert!(b.spawn(&b.script("true")).wait().unwrap().success());
    assert!(b.agent_started());
    assert!(!b.path("started.cred").exists());
}

#[test]
fn a_failed_install_stops_before_the_agent_and_removes_the_cred_dir() {
    let b = Scratch::new();
    std::fs::remove_file(b.path("home/.moor-cred-t1-x/brief.md")).unwrap();
    assert!(!b.spawn(&b.script("true")).wait().unwrap().success());
    assert!(!b.agent_started());
    assert!(b.cred_gone());
}

#[test]
fn a_failed_state_write_stops_before_the_agent_and_removes_the_cred_dir() {
    let b = Scratch::new();
    std::fs::create_dir_all(b.path("work/.moor/agent-state.json")).unwrap();
    assert!(!b.spawn(&b.script("true")).wait().unwrap().success());
    assert!(!b.agent_started());
    assert!(b.cred_gone());
}

#[test]
fn a_failed_hook_step_stops_before_the_agent_and_removes_the_cred_dir() {
    let b = Scratch::new();
    assert!(!b.spawn(&b.script("false")).wait().unwrap().success());
    assert!(!b.agent_started());
    assert!(b.cred_gone());
}

#[test]
fn a_hangup_before_the_brief_is_installed_removes_the_cred_dir() {
    let b = Scratch::new();
    b.stub("install", "touch \"$MARKER.install\"; sleep 30");
    let mut sh = b.spawn(&b.script("true"));
    while !b.path("started.install").exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let group = format!("-{}", sh.id());
    Command::new("kill")
        .args(["-HUP", "--", &group])
        .status()
        .unwrap();
    assert!(!sh.wait().unwrap().success());
    assert!(!b.agent_started());
    assert!(b.cred_gone());
}

fn proxy_addr() -> ProxyAddr {
    ProxyAddr {
        box_port: 4567,
        nonce: Secret::new(NONCE.to_vec()),
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        box_env: Vec::new(),
        forge: false,
    }
}

const NONCE: &[u8] = b"sentinel-9c2e";

fn req_with_proxy(s: &FakeSurface) -> LaunchReq {
    LaunchReq {
        proxy: Some(Arc::new(proxy_addr())),
        origin: Some("git@github.com:o/n.git".to_owned()),
        ..req(s)
    }
}

fn launch_with_proxy() -> (Arc<FakeDriver>, Arc<FakeSurface>) {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new("/r")));
    agent(&d, &s).launch(&req_with_proxy(&s)).unwrap();
    (d, s)
}

fn has_nonce(text: &str) -> bool {
    text.contains("sentinel-9c2e")
}

#[test]
fn the_nonce_reaches_the_box_only_through_its_staged_file() {
    let (d, s) = launch_with_proxy();
    assert!(s.opened().iter().all(|c| !has_nonce(&format!("{c:?}"))));
    assert!(s.calls().iter().all(|c| !has_nonce(c)));
    assert!(d.execs().iter().all(|e| !has_nonce(&format!("{e:?}"))));
    assert!(d.calls().iter().all(|c| !has_nonce(c)));
    let holders: Vec<_> = d
        .puts()
        .into_iter()
        .filter(|p| has_nonce(&String::from_utf8_lossy(&p.2)))
        .map(|p| (p.1.as_str().to_owned(), p.3))
        .collect();
    assert_eq!(holders, [(format!("{CRED}/proxy-nonce"), 0o600)]);
}

#[test]
fn a_proxy_launch_stages_only_the_nonce_and_the_public_ca() {
    let (d, _) = launch_with_proxy();
    let staged: Vec<_> = d
        .puts()
        .into_iter()
        .filter(|p| p.1.as_str().starts_with(&format!("{CRED}/")))
        .map(|p| (p.1.as_str().to_owned(), p.3))
        .collect();
    assert!(staged.contains(&(format!("{CRED}/proxy-nonce"), 0o600)));
    assert!(staged.contains(&(format!("{CRED}/proxy-ca.pem"), 0o644)));
    assert!(staged.iter().all(|(path, _)| !path.contains("/bin/")));
}

#[test]
fn a_proxy_launch_keeps_the_cred_dir_for_the_agent_and_exports_the_proxy_env() {
    let (_, s) = launch_with_proxy();
    let script = script_of(&pane_line(&s));
    assert!(!has_nonce(&script), "{script}");
    assert!(!script.contains(&format!("\nrm -rf '{CRED}'")), "{script}");
    for want in [
        "GIT_CONFIG_COUNT=2",
        "remote.moor.url",
        "HTTPS_PROXY",
        "SSL_CERT_FILE",
        "unset GH_TOKEN GITHUB_TOKEN",
    ] {
        assert!(script.contains(want), "{want}: {script}");
    }
}

#[test]
fn the_agent_gets_the_proxy_env_while_the_cred_dir_lives_until_exit() {
    let b = Scratch::new();
    let cred = b.path("home/.moor-cred-t1-x");
    std::fs::write(cred.join("proxy-nonce"), NONCE).unwrap();
    std::fs::write(cred.join("proxy-ca.pem"), proxy_addr().ca_pem).unwrap();
    b.stub(
        "claude",
        &format!(
            "touch \"$MARKER\"; env > \"$MARKER.env\"; \
             if [ -d '{c}' ]; then touch \"$MARKER.dir\"; fi; \
             if [ -e '{c}/proxy-nonce' ]; then touch \"$MARKER.nonce\"; fi",
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
    assert!(line("HTTPS_PROXY=").contains("sentinel-9c2e"), "{env}");
    assert!(!env.contains("GH_TOKEN"), "{env}");
    assert!(b.path("started.dir").exists());
    assert!(!b.path("started.nonce").exists());
    assert!(b.cred_gone());
}

mod batch;
mod env_tests;
mod proxy_launch_tests;
mod secret_tests;
