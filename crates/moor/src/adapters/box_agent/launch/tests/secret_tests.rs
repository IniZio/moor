//! The agent script hides the configured platform secrets from the agent and its children.

use super::*;

const NAMES: [&str; 2] = ["PLATFORM_TOKEN", "OTHER_TOKEN"];
const UNSET_LINE: &str = "unset 'PLATFORM_TOKEN' 'OTHER_TOKEN'";

fn names() -> Vec<String> {
    NAMES.map(String::from).to_vec()
}

fn launch_script(req: impl FnOnce(&FakeSurface) -> LaunchReq) -> String {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    agent(&d, &s).launch(&req(&s)).unwrap();
    script_of(&d)
}

fn with_names(s: &FakeSurface) -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        strip_env: names(),
        prompt: LaunchPrompt::None,
        ..req(s)
    }
}

fn assert_unsets_before_agent(script: &str) {
    let unset = script.find(UNSET_LINE).expect(script);
    let agent = script.rfind("'claude'").expect(script);
    assert!(unset < agent, "{script}");
}

#[test]
fn a_launch_without_a_proxy_unsets_the_configured_secrets_before_the_agent() {
    let script = launch_script(with_names);
    assert!(!script.contains("nono"), "{script}");
    assert_unsets_before_agent(&script);
}

#[test]
fn a_proxy_launch_unsets_the_configured_secrets_before_the_agent() {
    let script = launch_script(|s| LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        strip_env: names(),
        prompt: LaunchPrompt::None,
        ..req_with_proxy(s)
    });
    assert_unsets_before_agent(&script);
}

#[test]
fn the_unset_line_hides_the_configured_secrets_from_children() {
    let script = launch_script(with_names);
    let unset = script
        .lines()
        .find(|l| l.starts_with(UNSET_LINE))
        .expect(&script);
    let out = Command::new("sh")
        .args(["-c", &format!("{unset}\nenv")])
        .env("PLATFORM_TOKEN", "s1")
        .env("OTHER_TOKEN", "s2")
        .env("KEEP_ME", "kept")
        .output()
        .unwrap();
    let seen = String::from_utf8_lossy(&out.stdout);
    assert!(seen.contains("KEEP_ME=kept"), "{seen}");
    for name in NAMES {
        assert!(!seen.contains(name), "{name} leaked: {seen}");
    }
}

#[test]
fn an_empty_list_emits_no_unset_line() {
    let script = launch_script(req);
    assert!(!script.contains("PLATFORM_TOKEN"), "{script}");
}
