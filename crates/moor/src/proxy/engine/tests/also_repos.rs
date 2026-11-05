//! A Box that also holds a second repo: the engine pushes and opens PRs on both, and nothing else.

use moor_core::config::proxy::{resolve_rules, ProxyConfig, ResolvedRule};
use moor_core::seam::forge::ForgeRepo;
use ureq::Agent;

use super::graphql_e2e::{query, rig_spec, Rig, SECRET};
use crate::proxy::nono::tests::upstream::{self, Upstream};

/// What the client sends before the proxy swaps it out.
const PLACEHOLDER: &str = "Basic cGxhY2Vob2xkZXI=";

/// A running Box on `o/app` that also holds `o/lib`, with a fake host for the git rule.
struct Also {
    /// The engine and the fake `api.github.com`.
    rig: Rig,
    /// Fake `github.com`.
    git: Upstream,
}

fn repo(name: &str) -> ForgeRepo {
    ForgeRepo {
        owner: "o".to_owned(),
        name: name.to_owned(),
    }
}

/// The default GitHub rules for `o/app` and `o/lib`, the git host moved to `git_port`.
fn rules(git_port: u16) -> Vec<ResolvedRule> {
    let all = resolve_rules(&ProxyConfig::default(), &[repo("app"), repo("lib")], false).unwrap();
    all.into_iter()
        .filter(|r| matches!(r.name.as_str(), "github-api" | "github-git"))
        .map(|mut r| {
            if r.name == "github-git" {
                r.hosts = vec![format!("localhost:{git_port}")];
            }
            r
        })
        .collect()
}

fn also() -> Also {
    let dir = tempfile::tempdir().unwrap();
    let git = upstream::start(dir.path());
    let rig = rig_spec(repo("app"), vec![repo("lib")], rules(git.port), Some(&git));
    Also { rig, git }
}

/// POSTs an empty body to `path` on `api.github.com`; the status.
fn api_post(client: &Agent, path: &str) -> u16 {
    client
        .post(&format!("https://api.github.com{path}"))
        .header("Authorization", "token placeholder-not-real")
        .send("{}")
        .unwrap()
        .status()
        .as_u16()
}

impl Also {
    /// POSTs an empty body to `path` on the git host; the status.
    fn git_post(&self, path: &str) -> u16 {
        let url = format!("https://localhost:{}{path}", self.git.port);
        self.rig
            .client()
            .post(&url)
            .header("Authorization", PLACEHOLDER)
            .send("")
            .unwrap()
            .status()
            .as_u16()
    }

    /// The `Authorization` header of the one request the git host got.
    fn git_auth(&self) -> Option<String> {
        let seen = self.git.seen();
        let [only] = seen.as_slice() else {
            unreachable!("{seen:?}")
        };
        only.authorization.clone()
    }

    /// Whether an `egress_refused` event has been recorded.
    fn refused_recorded(&self) -> bool {
        let run = &self.rig.run;
        for _ in 0..100 {
            run.drain();
            if run.events().iter().any(|(k, _)| k == "egress_refused") {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    fn stop(self) {
        self.rig.stop();
    }
}

/// `Basic` of `x-access-token:` and the credential the rig hands out.
const BASIC_SECRET: &str = "Basic eC1hY2Nlc3MtdG9rZW46Z2gtZmFrZS10b2tlbi03Nw==";

#[test]
fn push_to_each_listed_repo_carries_the_credential() {
    for path in ["/o/app.git/git-receive-pack", "/o/lib.git/git-receive-pack"] {
        let a = also();
        assert_eq!(a.git_post(path), 200, "{path}");
        assert_eq!(a.git_auth(), Some(BASIC_SECRET.to_owned()), "{path}");
        a.stop();
    }
}

#[test]
fn push_to_an_unlisted_repo_is_refused_without_a_credential() {
    let a = also();
    let status = a.git_post("/o/other.git/git-receive-pack");
    assert!(matches!(status, 403 | 502), "{status}");
    assert!(a.git.seen().is_empty(), "{:?}", a.git.seen());
    assert!(a.refused_recorded(), "no egress_refused event");
    a.stop();
}

#[test]
fn fetch_from_an_unlisted_repo_is_allowed_without_a_credential() {
    let a = also();
    assert_eq!(a.git_post("/o/other.git/git-upload-pack"), 200);
    assert_eq!(a.git_auth(), Some(BASIC_SECRET.to_owned()));
    a.stop();
}

#[test]
fn pull_request_on_each_listed_repo_carries_the_credential() {
    for name in ["app", "lib"] {
        let a = also();
        let client = a.rig.client();
        assert_eq!(api_post(&client, &format!("/repos/o/{name}/pulls")), 200);
        let seen = a.rig.seen();
        let [only] = seen.as_slice() else {
            unreachable!("{seen:?}")
        };
        assert_eq!(only.line, format!("POST /repos/o/{name}/pulls HTTP/1.1"));
        assert_eq!(only.authorization, Some(format!("token {SECRET}")));
        a.stop();
    }
}

#[test]
fn pull_request_on_an_unlisted_repo_leaves_without_a_credential() {
    let a = also();
    let client = a.rig.client();
    assert_eq!(api_post(&client, "/repos/o/other/pulls"), 401);
    let seen = a.rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.authorization, None);
    assert!(!only.head.contains(SECRET), "{}", only.head);
    a.stop();
}

#[test]
fn graphql_scope_covers_each_listed_repo_and_no_other() {
    let a = also();
    let client = a.rig.client();
    for name in ["app", "lib"] {
        assert_eq!(query(&client, "o", name).0, 200, "{name}");
    }
    assert_eq!(query(&client, "o", "other").0, 401);
    let seen = a.rig.seen();
    let real = format!("token {SECRET}");
    let auth: Vec<_> = seen
        .iter()
        .map(|s| s.authorization.as_deref() == Some(real.as_str()))
        .collect();
    assert_eq!(auth, [true, true, false]);
    a.stop();
}
