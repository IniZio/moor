use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use parity::adapter::{EnvOpts, EnvReport, ExitClass, Outcome, StartOpts, TaskView};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{claude, herdr, Call, Rule};
use parity::parity;
use parity::world::config::RepoConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) const BASE: [&str; 17] = [
    "github.com",
    "api.github.com",
    "*.githubusercontent.com",
    "registry.npmjs.org",
    "mise.jdx.dev",
    "mise-versions.jdx.dev",
    "mise.run",
    "nodejs.org",
    "api.anthropic.com",
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    "download.docker.com",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];
pub(super) const PROXY_SERVED: [&str; 2] = ["api.github.com", "api.anthropic.com"];
pub(super) const NS_ENV: &str = "CRABBOX_NAMESPACE_EGRESS_DOMAINS";
pub(super) const SPRITES_FLAG: &str = "--sprites-network-allow";

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
}

pub(super) fn fx(preset: Preset, enforce: bool, user_extra: Value, repo_cfg: Value) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let mut bx = json!({ "provider": preset.provider() });
    if enforce {
        bx["enforce_egress"] = json!(true);
    }
    let mut user = json!({ "box": bx });
    for (k, v) in user_extra.as_object().into_iter().flatten() {
        user[k] = v.clone();
    }
    std::fs::write(w.config_path(), user.to_string()).expect("user config");
    std::fs::write(repo.path.join(".moor.yaml"), repo_cfg.to_string()).expect("repo config");
    crabbox::install_preset(&w, preset);
    herdr::install(&w);
    claude::install(&w);
    crabbox::ssh_channel(&w);
    crabbox::tunnel(&w);
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    Fx { w, repo }
}

pub(super) fn cfg(egress: &[&str]) -> Value {
    let mut v = RepoConfig::default().agent_location("box").to_value();
    v["egress"] = json!(egress);
    v
}

impl Fx {
    pub(super) fn set_repo(&self, doc: Value) {
        std::fs::write(self.repo.path.join(".moor.yaml"), doc.to_string()).expect("repo config");
    }

    pub(super) fn start(&self, base: &str, goal: Option<&str>) -> (String, Outcome<TaskView>) {
        self.start_with(base, goal, None)
    }

    pub(super) fn start_with(
        &self,
        base: &str,
        goal: Option<&str>,
        issue: Option<&str>,
    ) -> (String, Outcome<TaskView>) {
        let name = self.w.task_name(base);
        let opts = StartOpts {
            goal: goal.map(str::to_string),
            issue: issue.map(str::to_string),
            ..Default::default()
        };
        let o = moor().start(&self.w, &self.repo.path, &name, &opts);
        (name, o)
    }

    pub(super) fn start_ok(&self, base: &str, goal: Option<&str>) -> String {
        let (name, o) = self.start(base, goal);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        name
    }

    pub(super) fn warmups(&self) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some("warmup"))
            .collect()
    }

    pub(super) fn warmup(&self) -> Call {
        let mut all = self.warmups();
        assert_eq!(all.len(), 1, "exactly one warmup");
        all.remove(0)
    }

    pub(super) fn finish(&self, name: &str) {
        let closed = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let dir = self.w.state_dir().join("proxy");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            let live = std::fs::read_dir(&dir).is_ok_and(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "json"))
            });
            if !live {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("proxy supervisor did not exit after close");
    }
}

pub(super) fn csv(raw: &str) -> Vec<String> {
    raw.split(',').map(str::to_string).collect()
}

pub(super) fn sprites_list(c: &Call) -> Option<Vec<String>> {
    c.flag(SPRITES_FLAG).map(|v| csv(&v))
}

pub(super) fn ns_list(c: &Call) -> Option<Vec<String>> {
    c.env.get(NS_ENV).map(|v| csv(v))
}

pub(super) fn assert_set(list: &[String], expected: &BTreeSet<String>) {
    let got: BTreeSet<String> = list.iter().cloned().collect();
    assert_eq!(got.len(), list.len(), "duplicates in {list:?}");
    assert_eq!(&got, expected, "allow list differs");
}

pub(super) fn want(base_without: &[&str], extra: &[&str]) -> BTreeSet<String> {
    BASE.iter()
        .filter(|h| !base_without.contains(h))
        .chain(extra.iter())
        .map(|s| s.to_string())
        .collect()
}

pub(super) fn no_policy_anywhere(w: &World) {
    for c in w.calls("crabbox") {
        assert!(c.flag(SPRITES_FLAG).is_none(), "{:?}", c.argv);
        assert!(!c.env.contains_key(NS_ENV), "{:?}", c.argv);
    }
}

mod brief;
mod policy;
