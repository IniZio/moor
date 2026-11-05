//! Resolving rules for a Box of one or more repos.

use super::*;

#[test]
fn one_repo_resolves_exactly_as_before() {
    let cfg = user(GOOD);
    let rules = resolve_rules(&cfg.proxy, &[repo()], false).expect("resolves");
    let got = serde_json::to_string_pretty(&rules).expect("serializes") + "\n";
    if std::env::var_os("MOOR_BLESS").is_some() {
        let at = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/config/proxy/tests/one_repo.json"
        );
        std::fs::write(at, &got).expect("writes");
    }
    assert_eq!(got, include_str!("one_repo.json"));
}

fn app_and_lib() -> [ForgeRepo; 2] {
    [ForgeRepo::new("a", "app"), ForgeRepo::new("a", "lib")]
}

#[test]
fn every_listed_repo_may_push_and_open_prs() {
    let rules = resolve_rules(&ProxyConfig::default(), &app_and_lib(), false).expect("resolves");
    let entries: Vec<String> = rules
        .iter()
        .flat_map(|r| &r.endpoint_rules)
        .map(|e| format!("{} {}", e.method, e.path))
        .collect();
    for repo in ["a/app", "a/lib"] {
        for want in [
            format!("POST /{repo}.git/git-receive-pack"),
            format!("POST /repos/{repo}/pulls"),
        ] {
            assert!(entries.contains(&want), "{want} missing: {entries:?}");
        }
    }
    assert!(entries.iter().all(|e| !e.contains("a/other")));
}

#[test]
fn box_env_names_the_primary_repo() {
    let cfg = user(GOOD);
    let rules = resolve_rules(&cfg.proxy, &app_and_lib(), false).expect("resolves");
    let gitlab = rules.iter().find(|r| r.name == "gitlab").expect("gitlab");
    assert_eq!(
        gitlab.box_env.get("GL_REPO").map(String::as_str),
        Some("a/app")
    );
    let paths: Vec<&str> = gitlab
        .endpoint_rules
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert!(
        paths.contains(&"/api/v4/projects/a%2Flib/merge_requests/**"),
        "{paths:?}"
    );
}

fn git_entries(repos: &[ForgeRepo], fetch_without_repo: bool) -> Vec<String> {
    let rules =
        resolve_rules(&ProxyConfig::default(), repos, fetch_without_repo).expect("resolves");
    rules
        .iter()
        .find(|r| r.name == "github-git")
        .map(|r| {
            r.endpoint_rules
                .iter()
                .map(|e| format!("{} {}", e.method, e.path))
                .collect()
        })
        .unwrap_or_default()
}

fn git_allows(entries: &[String], method: &str, path: &str) -> bool {
    entries.iter().any(|e| {
        e.split_once(' ')
            .is_some_and(|(m, glob)| m == method && path_matches(glob, path))
    })
}

#[test]
fn any_repo_may_be_fetched_but_only_box_repos_pushed() {
    let entries = git_entries(&app_and_lib(), false);
    assert_eq!(
        entries,
        [
            "GET /*/*.git/info/refs",
            "POST /*/*.git/git-upload-pack",
            "POST /a/app.git/git-receive-pack",
            "POST /a/lib.git/git-receive-pack",
        ]
    );
    for (method, path, allowed) in [
        ("GET", "/other/thing.git/info/refs", true),
        ("POST", "/other/thing.git/git-upload-pack", true),
        ("GET", "/a/app.git/info/refs", true),
        ("POST", "/a/app.git/git-upload-pack", true),
        ("POST", "/a/app.git/git-receive-pack", true),
        ("POST", "/a/lib.git/git-receive-pack", true),
        ("POST", "/other/thing.git/git-receive-pack", false),
        ("GET", "/a/b/c.git/info/refs", false),
        ("POST", "/a/b/c.git/git-upload-pack", false),
        ("GET", "/thing.git/info/refs", false),
    ] {
        assert_eq!(
            git_allows(&entries, method, path),
            allowed,
            "{method} {path}"
        );
    }
}

#[test]
fn a_box_without_a_repo_fetches_only_when_it_has_a_profile() {
    assert_eq!(
        git_entries(&[], true),
        ["GET /*/*.git/info/refs", "POST /*/*.git/git-upload-pack"]
    );
    assert!(git_entries(&[], false).is_empty());
    let rules = resolve_rules(&ProxyConfig::default(), &[], true).expect("resolves");
    let names: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["claude", "github-git"]);
}

#[test]
fn any_repo_cannot_push() {
    let e = err("proxy:\n  rules:\n    github-git:\n      allow: ['POST /{any_repo}.git/git-receive-pack']\n");
    assert!(e.reason.contains("may only fetch"), "{e:?}");
}

#[test]
fn github_api_reads_any_repo_with_get_alone() {
    let rules = resolve_rules(&ProxyConfig::default(), &app_and_lib(), false).expect("resolves");
    let api = rules.iter().find(|r| r.name == "github-api").expect("api");
    let any_repo: Vec<String> = api
        .endpoint_rules
        .iter()
        .filter(|e| e.path.starts_with("/repos/*/*"))
        .map(|e| format!("{} {}", e.method, e.path))
        .collect();
    assert_eq!(
        any_repo,
        [
            "GET /repos/*/*",
            "GET /repos/*/*/releases",
            "GET /repos/*/*/releases/**",
        ]
    );
}

#[test]
fn a_box_without_a_repo_never_gets_the_github_api_rule() {
    let doc = "proxy:\n  rules:\n    github-api:\n      allow: ['GET /repos/{any_repo}/tags']\n";
    let extended = user(doc);
    for cfg in [&ProxyConfig::default(), &extended.proxy] {
        let rules = resolve_rules(cfg, &[], true).expect("resolves");
        let names: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["claude", "github-git"]);
    }
}
