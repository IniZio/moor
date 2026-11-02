//! Resolving rules for a Box of one or more repos.

use super::*;

#[test]
fn one_repo_resolves_exactly_as_before() {
    let cfg = user(GOOD);
    let rules = resolve_rules(&cfg.proxy, &[repo()], &reads()).expect("resolves");
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
    let rules = resolve_rules(&ProxyConfig::default(), &app_and_lib(), &[]).expect("resolves");
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
    let rules = resolve_rules(&cfg.proxy, &app_and_lib(), &[]).expect("resolves");
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
