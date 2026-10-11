use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{moor, world};
use parity::adapter::github_ops::{GithubSetupOpts, GithubStore};
use parity::adapter::{EnvOpts, EventKind, ExitClass, Outcome, StartOpts};
use parity::assert::{assert_never_called, assert_pure_json, assert_secret_absent};
use parity::fake::{claude, crabbox, herdr, Rule};
use parity::github::{FakeGithub, Reply, RepoFixture, Request, TEST_KEY_PEM};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

mod box_flow;
mod http;
mod identity;
mod login;
mod mint;
mod setup;

pub(super) struct Gx {
    pub(super) w: World,
    pub(super) repo: Repo,
    pub(super) gh: FakeGithub,
    pub(super) fx: RepoFixture,
}

pub(super) fn gx() -> Gx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let gh = FakeGithub::start();
    gh.attach(&w);
    let fx = RepoFixture::new("acme", "widget");
    for s in [
        &fx.user_access,
        &fx.user_refresh,
        &fx.scoped_token,
        &fx.bot_token,
        &fx.probe_token,
    ] {
        w.register_secret(s);
    }
    for s in ["JUNKKEYMARKER", "ghp_HOSTTOKEN", "ghr_NEW"] {
        w.register_secret(s);
    }
    if let Some(body) = TEST_KEY_PEM.lines().nth(1) {
        w.register_secret(body);
    }
    Gx { w, repo, gh, fx }
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

pub(super) fn write_file(path: &Path, body: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

pub(super) fn snap(o: &Outcome<GithubStore>) -> &GithubStore {
    o.value.as_ref().expect("store snapshot")
}

pub(super) fn seq(reqs: &[Request]) -> Vec<String> {
    reqs.iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

pub(super) fn assert_subseq(reqs: &[Request], want: &[&str]) {
    let got = seq(reqs);
    let mut it = got.iter();
    for w in want {
        assert!(
            it.any(|g| g == w),
            "request {w} missing or out of order in {got:?}"
        );
    }
}

pub(super) fn count(reqs: &[Request], method: &str, path: &str) -> usize {
    reqs.iter()
        .filter(|r| r.method == method && r.path == path)
        .count()
}

pub(super) fn permissions_write() -> Value {
    json!({ "contents": "write", "pull_requests": "write", "workflows": "write" })
}

impl Gx {
    pub(super) fn secret(&self, tag: &str) -> String {
        let secret = format!("SECRET-{tag}-{}", self.w.task_name("v"));
        self.w.register_secret(&secret);
        secret
    }

    pub(super) fn user(&self, doc: Value) {
        write_file(&self.w.config_path(), &doc.to_string(), 0o644);
    }

    pub(super) fn identity(&self, identity: &str) {
        self.user(json!({ "github": { "identity": identity } }));
    }

    pub(super) fn git(&self, args: &[&str]) {
        let st = self
            .w
            .command("git")
            .current_dir(&self.repo.path)
            .args(args)
            .status()
            .expect("git");
        assert!(st.success(), "git {args:?}");
    }

    pub(super) fn github_origin(&self) {
        let key = format!("url.{}.insteadOf", self.repo.bare.display());
        let st = self
            .w
            .command("git")
            .args(["config", "--global", "--unset", &key])
            .status()
            .expect("git config");
        assert!(st.success());
    }

    pub(super) fn key(&self, mode: u32) -> PathBuf {
        let path = self.w.root().join(format!("app-key-{mode:o}.pem"));
        write_file(&path, TEST_KEY_PEM, mode);
        path
    }

    pub(super) fn opts(&self, secret: &str) -> GithubSetupOpts {
        GithubSetupOpts {
            app_id: Some("4242".into()),
            client_id: Some(self.fx.client_id.clone()),
            key: Some(self.key(0o600)),
            repo: Some("acme/widget".into()),
            secret_env: Some(secret.to_string()),
            ..Default::default()
        }
    }

    pub(super) fn setup_with(&self, opts: &GithubSetupOpts) -> Outcome<GithubStore> {
        moor().github_setup(&self.w, &self.repo.path, opts)
    }

    pub(super) fn setup(&self) -> String {
        let secret = self.secret("clientsecret");
        let o = self.setup_with(&self.opts(&secret));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        secret
    }

    pub(super) fn login(&self) -> Outcome<GithubStore> {
        moor().github_login(&self.w, &self.repo.path)
    }

    pub(super) fn store(&self) -> GithubStore {
        GithubStore::read(&self.w)
    }

    pub(super) fn wipe_store(&self) {
        let _ = std::fs::remove_dir_all(self.w.state_dir().join("github"));
    }

    pub(super) fn seed_user(&self, refresh: &str, access: Option<(&str, u64)>) {
        let mut doc = json!({ "refresh_token": refresh });
        if let Some((token, expires_at)) = access {
            doc["access_token"] = json!(token);
            doc["access_expires_at"] = json!(expires_at);
        }
        let path = self.w.state_dir().join("github/user.json");
        write_file(&path, &doc.to_string(), 0o600);
    }

    pub(super) fn env_json(&self) -> Value {
        let o = moor().env(&self.w, &self.repo.path, &EnvOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        serde_json::from_str(&o.stdout).expect("env json")
    }

    pub(super) fn mint(&self) -> Value {
        self.env_json()["github"]["mint"].clone()
    }

    pub(super) fn mint_status(&self) -> String {
        self.mint()["status"].as_str().unwrap_or("").to_string()
    }

    pub(super) fn mint_error(&self) -> String {
        self.mint()["error"].as_str().unwrap_or("").to_string()
    }

    pub(super) fn resolved(&self) -> Value {
        self.env_json()["github"]["resolved"].clone()
    }

    pub(super) fn installation(&self, status: u16) {
        let reply = match status {
            200 => Reply::ok(&json!({ "id": self.fx.installation_id })),
            s => Reply::message(s, "Not Found"),
        };
        let path = format!("{}/installation", self.fx.repo_path());
        self.gh.reroute("GET", &path, vec![reply]);
    }

    pub(super) fn gh_token_calls(&self) -> usize {
        self.w
            .calls("gh")
            .iter()
            .filter(|c| c.argv == ["auth", "token"])
            .count()
    }

    pub(super) fn no_gh_fallback(&self) {
        assert_never_called(&self.w.all_calls(), "gh", &["auth"]);
    }
}

pub(super) fn app_you_world() -> Gx {
    let g = gx();
    g.github_origin();
    g.gh.script_repo(&g.fx);
    g
}
