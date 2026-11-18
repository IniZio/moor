#![allow(clippy::indexing_slicing, reason = "test assertions")]
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use moor_core::name::BoxPath;
use moor_core::profile::{self, Manifest};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::Claude;
use crate::adapters::box_agent::profile_writer::{ProfileCtx, ProfileWriter};

const FULL: &str = "claude:\n  marketplaces:\n    tools: https://github.com/acme/agent-tools\n  \
plugins: [lint@tools, fmt@tools]\n";

/// A Box laid out under a temp dir, with a `claude` on `PATH` that logs its arguments, and a
/// `sleep` that logs its pause and returns at once.
struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        for d in ["home", "work", "profile", "bin"] {
            fs::create_dir_all(root.path().join(d)).unwrap();
        }
        let claude = root.path().join("bin/claude");
        let script = "#!/bin/sh\necho \"$*\" >> \"$STUB_LOG\"\n\
[ \"$*\" = \"plugin install $STUB_FAIL\" ] && exit 7\n\
if [ -n \"$STUB_FLAKY\" ] && [ \"$*\" = \"$STUB_FLAKY\" ]; then \
n=$(cat \"$STUB_LOG.flaky\" 2>/dev/null || echo 0); n=$((n + 1)); echo $n > \"$STUB_LOG.flaky\"; \
[ $n -le 2 ] && exit 5; fi\nexit 0\n";
        fs::write(&claude, script).unwrap();
        fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let sleep = root.path().join("bin/sleep");
        fs::write(&sleep, "#!/bin/sh\necho \"sleep $*\" >> \"$STUB_LOG\"\n").unwrap();
        fs::set_permissions(&sleep, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.root.path().join(rel)
    }

    fn put(&self, rel: &str, text: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn run(&self, manifest: &Manifest, fail: &str) -> Output {
        self.run_flaky(manifest, fail, "")
    }

    /// [`Self::run`] with the stub failing the first two calls whose arguments are `flaky`.
    fn run_flaky(&self, manifest: &Manifest, fail: &str, flaky: &str) -> Output {
        let box_path = |rel| BoxPath::under(self.root.path().to_str().unwrap(), rel).unwrap();
        let (dir, workdir, home) = (box_path("profile"), box_path("work"), box_path("home"));
        let setup = Claude
            .write(&ProfileCtx {
                manifest,
                profile_dir: &dir,
                workdir: &workdir,
                home: &home,
            })
            .unwrap();
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new("/bin/sh")
            .args([
                "-c",
                &format!("{}\n{}", setup.step, Claude.installs(manifest)),
            ])
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("STUB_LOG", self.path("log"))
            .env("STUB_FAIL", fail)
            .env("STUB_FLAKY", flaky)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn has_interpreter() -> bool {
    ["python3", "node"].iter().any(|t| {
        Command::new("/bin/sh")
            .args(["-c", &format!("command -v {t}")])
            .output()
            .unwrap()
            .status
            .success()
    })
}

fn golden(name: &str, manifest: &Manifest) {
    let at = |p: &str| BoxPath::under("/box", p).unwrap();
    let (dir, workdir, home) = (at("home/.moor-profile/person"), at("work"), at("home"));
    let step = Claude
        .write(&ProfileCtx {
            manifest,
            profile_dir: &dir,
            workdir: &workdir,
            home: &home,
        })
        .unwrap()
        .step;
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/adapters/box_agent/claude/profile/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&file, &step).unwrap();
    }
    assert_eq!(step, fs::read_to_string(&file).unwrap(), "{name}");
}

#[test]
fn empty_profile_script_is_stable() {
    golden("empty.sh", &Manifest::default());
}

#[test]
fn full_profile_script_is_stable() {
    golden("full.sh", &profile::parse(FULL).unwrap());
}

#[test]
fn instructions_become_claude_md_and_absent_ones_write_nothing() {
    let f = Fixture::new();
    assert!(f.run(&Manifest::default(), "").status.success());
    assert!(!f.path("home/.claude/CLAUDE.md").exists());
    f.put("profile/AGENTS.md", "be kind\n");
    assert!(f.run(&Manifest::default(), "").status.success());
    assert_eq!(f.read("home/.claude/CLAUDE.md"), "be kind\n");
}

#[test]
fn repo_skill_shadows_a_person_skill_of_the_same_name() {
    let f = Fixture::new();
    f.put("profile/skills/shared/SKILL.md", "person shared");
    f.put("profile/skills/mine/SKILL.md", "person mine");
    f.put("profile/skills/plainfolder/readme", "x");
    f.put("work/.agents/skills/shared/SKILL.md", "repo shared");
    f.put("work/.agents/skills/theirs/SKILL.md", "repo theirs");
    for _ in 0..2 {
        assert!(f.run(&Manifest::default(), "").status.success());
        assert_eq!(f.read("home/.claude/skills/shared/SKILL.md"), "repo shared");
        assert_eq!(f.read("home/.claude/skills/mine/SKILL.md"), "person mine");
        assert_eq!(f.read("home/.claude/skills/theirs/SKILL.md"), "repo theirs");
        assert!(!f.path("home/.claude/skills/plainfolder").exists());
    }
}

#[test]
fn settings_fragment_merges_and_existing_values_win() {
    if !has_interpreter() {
        return;
    }
    let f = Fixture::new();
    let moor_hook =
        json!({"hooks": [{"type": "command", "command": "sh /w/.moor/hook.sh idle Stop"}]});
    f.put(
        "home/.claude/settings.json",
        &json!({
            "enableAllProjectMcpServers": true,
            "userKey": 1,
            "hooks": {"Stop": [moor_hook]},
        })
        .to_string(),
    );
    let theirs = json!({"hooks": [{"type": "command", "command": "echo profile"}]});
    f.put(
        "profile/claude/settings.json",
        &json!({
            "enableAllProjectMcpServers": false,
            "env": {"A": "b"},
            "hooks": {"Stop": [theirs, moor_hook]},
        })
        .to_string(),
    );
    for _ in 0..2 {
        assert!(f.run(&Manifest::default(), "").status.success());
        let got: Value = serde_json::from_str(&f.read("home/.claude/settings.json")).unwrap();
        assert_eq!(got["enableAllProjectMcpServers"], true);
        assert_eq!(got["userKey"], 1);
        assert_eq!(got["env"]["A"], "b");
        assert_eq!(got["hooks"]["Stop"], json!([moor_hook, theirs]));
    }
}

#[test]
fn marketplaces_are_added_before_plugins_are_installed() {
    let f = Fixture::new();
    assert!(f.run(&profile::parse(FULL).unwrap(), "").status.success());
    assert_eq!(
        f.read("log"),
        "plugin marketplace add https://github.com/acme/agent-tools\n\
plugin install lint@tools\nplugin install fmt@tools\n"
    );
    assert_eq!(f.read("work/.moor/profile-failures"), "");
}

#[test]
fn a_failed_install_is_recorded_and_the_rest_still_run() {
    let f = Fixture::new();
    let out = f.run(&profile::parse(FULL).unwrap(), "lint@tools");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "moor: profile: plugin lint@tools exit 7\n"
    );
    assert_eq!(
        f.read("work/.moor/profile-failures"),
        "plugin lint@tools exit 7\n"
    );
    assert!(f.read("log").ends_with("plugin install fmt@tools\n"));
}

#[test]
fn the_profile_marketplace_is_the_checkout_and_plugins_install_under_its_own_name() {
    if !has_interpreter() {
        return;
    }
    let f = Fixture::new();
    f.put(
        "profile/.claude-plugin/marketplace.json",
        r#"{"name": "mine", "plugins": [{"name": "x", "source": "./x"}]}"#,
    );
    let m = profile::parse("claude:\n  plugins: [lint@profile]\n").unwrap();
    assert!(f.run(&m, "").status.success());
    assert_eq!(
        f.read("log"),
        format!(
            "plugin marketplace add {}\nplugin install lint@mine\n",
            f.path("profile").display()
        )
    );
    assert_eq!(f.read("work/.moor/profile-failures"), "");
}

#[test]
fn a_marketplace_ref_is_kept_in_the_added_source() {
    let f = Fixture::new();
    let m = profile::parse(
        "claude:\n  marketplaces:\n    tools: https://github.com/acme/agent-tools#feat/x\n",
    )
    .unwrap();
    assert!(f.run(&m, "").status.success());
    assert_eq!(
        f.read("log"),
        "plugin marketplace add https://github.com/acme/agent-tools#feat/x\n"
    );
}

#[test]
fn a_flaky_install_is_tried_again_after_10_then_30_seconds_and_passes() {
    let f = Fixture::new();
    let out = f.run_flaky(
        &profile::parse(FULL).unwrap(),
        "",
        "plugin install lint@tools",
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(
        f.read("log"),
        "plugin marketplace add https://github.com/acme/agent-tools\n\
plugin install lint@tools\nsleep 10\nplugin install lint@tools\nsleep 30\n\
plugin install lint@tools\nplugin install fmt@tools\n"
    );
    assert_eq!(f.read("work/.moor/profile-failures"), "");
}

#[test]
fn a_flaky_marketplace_add_is_tried_again_and_its_plugins_install() {
    let f = Fixture::new();
    let add = "plugin marketplace add https://github.com/acme/agent-tools";
    let out = f.run_flaky(&profile::parse(FULL).unwrap(), "", add);
    assert!(out.status.success());
    assert_eq!(
        f.read("log"),
        format!("{add}\nsleep 10\n{add}\nsleep 30\n{add}\nplugin install lint@tools\nplugin install fmt@tools\n")
    );
    assert_eq!(f.read("work/.moor/profile-failures"), "");
}

#[test]
fn an_install_that_fails_three_times_is_recorded_once() {
    let f = Fixture::new();
    let out = f.run(&profile::parse(FULL).unwrap(), "lint@tools");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "moor: profile: plugin lint@tools exit 7\n"
    );
    let log = f.read("log");
    let tries = log
        .lines()
        .filter(|l| *l == "plugin install lint@tools")
        .count();
    assert_eq!(tries, 3, "{log}");
    assert!(
        log.contains("lint@tools\nsleep 10\nplugin install lint@tools\nsleep 30\n"),
        "{log}"
    );
    assert_eq!(
        f.read("work/.moor/profile-failures"),
        "plugin lint@tools exit 7\n"
    );
}
