#![allow(clippy::indexing_slicing, reason = "test assertions")]
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use moor_core::name::BoxPath;
use moor_core::profile::{self, Manifest};
use tempfile::TempDir;

use super::super::Codex;
use crate::adapters::box_agent::profile_writer::{ProfileCtx, ProfileWriter};

const FULL: &str =
    "codex:\n  model: gpt-5.5\n  marketplaces:\n    tools: https://github.com/acme/agent-tools\n  \
plugins: [lint@tools, fmt@profile]\n";

/// A Box laid out under a temp dir, with a `codex` on `PATH` that logs its arguments.
struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        for d in ["home", "work", "profile", "bin"] {
            fs::create_dir_all(root.path().join(d)).unwrap();
        }
        let codex = root.path().join("bin/codex");
        let script = "#!/bin/sh\necho \"$*\" >> \"$STUB_LOG\"\n\
[ \"$*\" = \"plugin add $STUB_FAIL\" ] && exit 7\n\
case \"$*\" in *--json*) echo '{\"marketplaceName\": \"local-mk\"}';; esac\nexit 0\n";
        fs::write(&codex, script).unwrap();
        fs::set_permissions(&codex, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
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

    fn config(&self) -> toml::Value {
        self.read("home/.codex/config.toml").parse().unwrap()
    }

    fn run(&self, manifest: &Manifest, fail: &str) -> Output {
        let box_path = |rel| BoxPath::under(self.root.path().to_str().unwrap(), rel).unwrap();
        let (dir, workdir, home) = (box_path("profile"), box_path("work"), box_path("home"));
        let setup = Codex
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
            .args(["-c", &setup.step])
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("STUB_LOG", self.path("log"))
            .env("STUB_FAIL", fail)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn model_only() -> Manifest {
    profile::parse("codex:\n  model: gpt-5.5\n").unwrap()
}

#[test]
fn instructions_and_skills_land_where_codex_reads_them() {
    let f = Fixture::new();
    assert!(f.run(&Manifest::default(), "").status.success());
    assert!(!f.path("home/.codex/AGENTS.md").exists());
    f.put("profile/AGENTS.md", "be kind\n");
    f.put("profile/skills/mine/SKILL.md", "person mine");
    f.put("profile/skills/plainfolder/readme", "x");
    for _ in 0..2 {
        assert!(f.run(&Manifest::default(), "").status.success());
        assert_eq!(f.read("home/.codex/AGENTS.md"), "be kind\n");
        assert_eq!(f.read("home/.agents/skills/mine/SKILL.md"), "person mine");
        assert!(!f.path("home/.agents/skills/plainfolder").exists());
    }
}

fn assert_user_lines_kept_once(text: &str) {
    assert_eq!(text.matches("# >>> moor-profile >>>").count(), 1, "{text}");
    assert_eq!(text.matches("# <<< moor-profile <<<").count(), 1, "{text}");
    assert!(text.contains("approval_policy = \"never\"\n"), "{text}");
    assert!(
        text.contains("[projects.\"/w\"]\ntrust_level = \"trusted\"\n"),
        "{text}"
    );
}

#[test]
fn the_block_is_replaced_not_duplicated_and_user_lines_stay() {
    let f = Fixture::new();
    f.put(
        "home/.codex/config.toml",
        "approval_policy = \"never\"\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n",
    );
    for _ in 0..3 {
        assert!(f.run(&model_only(), "").status.success());
    }
    let text = f.read("home/.codex/config.toml");
    assert_user_lines_kept_once(&text);
    f.put(
        "profile/codex/config.toml",
        "[features]\nshell_tool = true\n",
    );
    assert!(f.run(&Manifest::default(), "").status.success());
    let text = f.read("home/.codex/config.toml");
    assert!(!text.contains("gpt-5.5"), "{text}");
    assert!(text.contains("shell_tool = true"), "{text}");
    assert_eq!(text.matches("# >>> moor-profile >>>").count(), 1, "{text}");
}

#[test]
fn model_stays_top_level_when_the_file_has_tables() {
    let f = Fixture::new();
    f.put(
        "home/.codex/config.toml",
        "[projects.\"/w\"]\ntrust_level = \"trusted\"\n",
    );
    f.put(
        "profile/codex/config.toml",
        "model = \"other\"\nsandbox_mode = \"read-only\"\n[features]\nx = true\n",
    );
    assert!(f.run(&model_only(), "").status.success());
    let cfg = f.config();
    assert_eq!(cfg["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(cfg["sandbox_mode"].as_str(), Some("read-only"));
    assert_eq!(cfg["features"]["x"].as_bool(), Some(true));
    assert_eq!(
        cfg["projects"]["/w"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert!(cfg["projects"].get("model").is_none());
}

#[test]
fn a_model_with_quotes_is_escaped() {
    let f = Fixture::new();
    let m = profile::parse("codex:\n  model: 'a\"b\\c'\n").unwrap();
    assert!(f.run(&m, "").status.success());
    assert_eq!(f.config()["model"].as_str(), Some("a\"b\\c"));
}

#[test]
fn marketplaces_are_added_before_plugins_and_profile_means_the_checkout() {
    let f = Fixture::new();
    assert!(f.run(&profile::parse(FULL).unwrap(), "").status.success());
    assert_eq!(
        f.read("log"),
        format!(
            "plugin marketplace add https://github.com/acme/agent-tools\n\
plugin marketplace add --json {}\nplugin add lint@tools\nplugin add fmt@local-mk\n",
            f.path("profile").display()
        )
    );
    assert_eq!(f.read("work/.moor/profile-failures"), "");
}

#[test]
fn a_failed_plugin_is_recorded_and_the_rest_still_run() {
    let f = Fixture::new();
    let out = f.run(&profile::parse(FULL).unwrap(), "lint@tools");
    assert!(out.status.success());
    assert_eq!(
        f.read("work/.moor/profile-failures"),
        "plugin lint@tools exit 7\n"
    );
    assert!(f.read("log").ends_with("plugin add fmt@local-mk\n"));
}

#[test]
fn fragment_tables_never_swallow_the_persons_later_top_level_keys() {
    let f = Fixture::new();
    f.put(
        "home/.codex/config.toml",
        "approval_policy = \"never\"\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n",
    );
    f.put(
        "profile/codex/config.toml",
        "sandbox_mode = \"read-only\"\n[features]\nx = true\n[tui]\ny = 1\n",
    );
    assert!(f.run(&model_only(), "").status.success());
    let first = f.read("home/.codex/config.toml");
    assert_fragment_in_place(&f.config());
    assert!(f.run(&model_only(), "").status.success());
    assert_fragment_in_place(&f.config());
    assert_eq!(f.read("home/.codex/config.toml"), first);
    let (head, tail) = first.split_once("# <<< moor-profile <<<\n").unwrap();
    f.put(
        "home/.codex/config.toml",
        &format!("{head}# <<< moor-profile <<<\nlater = 1\n{tail}"),
    );
    assert!(f.run(&model_only(), "").status.success());
    let cfg = f.config();
    assert_eq!(cfg["later"].as_integer(), Some(1));
    assert!(cfg["tui"].get("later").is_none());
}

fn assert_fragment_in_place(cfg: &toml::Value) {
    assert_eq!(cfg["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(cfg["sandbox_mode"].as_str(), Some("read-only"));
    assert_eq!(cfg["approval_policy"].as_str(), Some("never"));
    assert_eq!(cfg["features"]["x"].as_bool(), Some(true));
    assert_eq!(cfg["tui"]["y"].as_integer(), Some(1));
    assert_eq!(
        cfg["projects"]["/w"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert!(cfg["projects"]["/w"].get("sandbox_mode").is_none());
}
