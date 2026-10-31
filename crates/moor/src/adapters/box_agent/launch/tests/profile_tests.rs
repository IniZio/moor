//! A Box with a person profile applies it in the agent script, after the hooks and preseed and
//! before the agent; a Box without one gets the script it always had.

use moor_core::profile::parse;

use super::*;
use crate::adapters::box_agent::launch::AGENT_PATH_LINE;
use crate::adapters::box_agent::profile_writer::FAILURES_FILE;

const MANIFEST: &str =
    "claude:\n  marketplaces:\n    m: https://github.com/acme/market\n  plugins:\n    - p@m\n";

fn launched(req: impl FnOnce(&FakeSurface) -> LaunchReq) -> String {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    agent(&d, &s).launch(&req(&s)).unwrap();
    script_of(&d)
}

fn at(script: &str, needle: &str) -> usize {
    script.find(needle).expect(needle)
}

#[test]
fn the_profile_writers_run_after_the_path_and_proxy_and_before_the_agent() {
    let script = launched(|s| LaunchReq {
        profile: Some(parse(MANIFEST).unwrap()),
        ..req_with_proxy(s)
    });
    let starting = at(&script, "\"state\":\"starting\"");
    let path = at(&script, AGENT_PATH_LINE);
    let proxy = at(&script, "HTTPS_PROXY");
    let truncate = at(&script, &format!(": > '/work/box-1/{FAILURES_FILE}.all'"));
    let claude = at(&script, "plugin install 'p@m'");
    let codex = at(&script, "/.codex'");
    let kept = at(&script, &format!("mv '/work/box-1/{FAILURES_FILE}.all'"));
    let agent = script.rfind("'claude'").unwrap();
    assert!(kept < agent && codex < kept, "{script}");
    assert!(
        starting < path && path < proxy && proxy < truncate,
        "{script}"
    );
    assert!(
        truncate < claude && claude < codex && codex < agent,
        "{script}"
    );
}

#[test]
fn a_box_without_a_profile_runs_no_writer() {
    let script = launched(req_with_proxy);
    assert!(!script.contains(FAILURES_FILE), "{script}");
    assert!(!script.contains(".moor-profile"), "{script}");
}

#[test]
fn the_profile_env_is_exported_after_the_proxy_and_before_the_writers_and_the_agent() {
    let manifest = "claude:\n  env:\n    BUN_OPTIONS: \"--preload={profile}/claude/x.js\"\n    NOTE: \"$(touch x)\"\n";
    let script = launched(|s| LaunchReq {
        profile: Some(parse(manifest).unwrap()),
        ..req_with_proxy(s)
    });
    let proxy = at(&script, "HTTPS_PROXY");
    let bun = at(
        &script,
        "export BUN_OPTIONS='--preload=/home/box/.moor-profile/person/claude/x.js'",
    );
    let note = at(&script, "export NOTE='$(touch x)'");
    let truncate = at(&script, &format!(": > '/work/box-1/{FAILURES_FILE}.all'"));
    assert!(proxy < bun && bun < note && note < truncate, "{script}");
}

#[test]
fn the_repo_env_is_exported_after_the_profile_env() {
    let script = launched(|s| LaunchReq {
        profile: Some(parse("claude:\n  env: {A: profile}\n").unwrap()),
        proxy: Some(Arc::new(ProxyAddr {
            box_env: vec![("A".to_owned(), "repo".to_owned())],
            ..proxy_addr()
        })),
        ..req_with_proxy(s)
    });
    assert!(
        at(&script, "export A='profile'") < at(&script, "export A='repo'"),
        "{script}"
    );
}

#[test]
fn a_profile_without_env_exports_nothing_extra() {
    let script = launched(|s| LaunchReq {
        profile: Some(parse(MANIFEST).unwrap()),
        ..req_with_proxy(s)
    });
    assert!(!script.contains("BUN_OPTIONS"), "{script}");
}
