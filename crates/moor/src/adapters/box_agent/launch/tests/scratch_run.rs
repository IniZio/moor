use moor_core::activity::HookKind;

use super::*;

/// A scratch Box: home, workdir, a credential dir holding a brief, and a stub agent.
pub(super) struct Scratch {
    egress: moor_testkit::FakeEgress,
    dir: tempfile::TempDir,
}

impl Scratch {
    pub(super) fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        for d in ["home", "work", "bin", "home/.moor-cred-t1-x"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        moor_testkit::write_host_guards(&dir.path().join("bin")).unwrap();
        let egress =
            moor_testkit::FakeEgress::start(&dir.path().join("home"), &dir.path().join("bin"))
                .unwrap();
        let s = Self { egress, dir };
        std::fs::write(s.path("home/.moor-cred-t1-x/brief.md"), "brief").unwrap();
        s.stub("claude", "touch \"$MARKER\"");
        s
    }

    pub(super) fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub(super) fn stub(&self, name: &str, body: &str) {
        let p = self.path(&format!("bin/{name}"));
        moor_testkit::write_executable(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    }

    pub(super) fn script(&self, hook_step: &str) -> String {
        self.script_with(hook_step, &[])
    }

    pub(super) fn proxy_ctx(&self) -> LaunchCtx {
        let root = self.dir.path().to_str().unwrap();
        LaunchCtx {
            model: None,
            workdir: BoxPath::under(root, "work").unwrap(),
            proxy: Some(self.proxy_addr()),
            home: BoxPath::under(root, "home").unwrap(),
            prompt: LaunchPrompt::Brief,
        }
    }

    /// The Proxy address whose egress port the fake tinyproxy opens.
    pub(super) fn proxy_addr(&self) -> ProxyAddr {
        ProxyAddr {
            egress_port: self.egress.port(),
            ..proxy_addr()
        }
    }

    pub(super) fn proxy_lines(&self) -> Vec<String> {
        let env = Claude.env(&self.proxy_ctx());
        proxy_env::script_lines(&self.proxy_addr(), &env).unwrap()
    }

    pub(super) fn script_with(&self, hook_step: &str, proxy: &[String]) -> String {
        let root = self.dir.path().to_str().unwrap();
        let ctx = LaunchCtx {
            model: None,
            workdir: BoxPath::under(root, "work").unwrap(),
            proxy: None,
            home: BoxPath::under(root, "home").unwrap(),
            prompt: LaunchPrompt::Brief,
        };
        let setup = |step: &str| BoxSetup {
            files: Vec::new(),
            step: step.to_owned(),
        };
        let cred = BoxPath::under(root, "home/.moor-cred-t1-x").unwrap();
        let tmp = BoxPath::under(root, "home/.moor-tmp-t1-x").unwrap();
        super::super::script(
            (&cred, &tmp, "x"),
            &ctx,
            &setup(hook_step),
            &setup("true"),
            &[],
            (proxy, &[]),
        )
        .unwrap()
    }

    pub(super) fn spawn(&self, script: &str) -> std::process::Child {
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

    pub(super) fn cred_gone(&self) -> bool {
        !self.path("home/.moor-cred-t1-x").exists()
    }

    pub(super) fn agent_started(&self) -> bool {
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
fn the_starting_marker_names_this_launch() {
    let b = Scratch::new();
    assert!(b.spawn(&b.script("true")).wait().unwrap().success());
    let raw = std::fs::read(b.path("work/.moor/agent-state.json")).unwrap();
    let marker = Claude.parse_hook(&raw).map(|r| r.kind);
    assert_eq!(
        marker,
        Some(HookKind::Starting {
            launch: Some("x".to_owned())
        })
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
