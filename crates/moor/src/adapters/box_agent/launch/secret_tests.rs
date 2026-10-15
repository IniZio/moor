//! The agent script hides the platform secrets from the agent and its children.

use super::*;

const UNSET_LINE: &str = "unset AGENT37_STARTER_TOKEN OPENCLAW_STARTER_TOKEN AGENT37_MANAGED_TOKEN";

fn assert_unsets_before_agent(script: &str) {
    let unset = script.find(UNSET_LINE).expect(script);
    let agent = script.rfind("'claude'").expect(script);
    assert!(unset < agent, "{script}");
}

#[test]
fn an_unsandboxed_script_unsets_the_platform_secrets_before_the_agent() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let script = script_of(&pane_line(&s));
    assert!(!script.contains("nono"), "{script}");
    assert_unsets_before_agent(&script);
}

#[test]
fn a_sandboxed_script_unsets_the_platform_secrets_before_the_agent() {
    let (_, s) = launch_with_proxy();
    let script = script_of(&pane_line(&s));
    assert!(script.contains("nono"), "{script}");
    assert_unsets_before_agent(&script);
}

#[test]
fn the_unset_line_hides_the_platform_secrets_from_children() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let script = script_of(&pane_line(&s));
    let unset = script
        .lines()
        .find(|l| l.starts_with("unset "))
        .expect(&script);
    let out = Command::new("sh")
        .args(["-c", &format!("{unset}\nenv")])
        .env("AGENT37_STARTER_TOKEN", "s1")
        .env("OPENCLAW_STARTER_TOKEN", "s2")
        .env("AGENT37_MANAGED_TOKEN", "s3")
        .env("KEEP_ME", "kept")
        .output()
        .unwrap();
    let seen = String::from_utf8_lossy(&out.stdout);
    assert!(seen.contains("KEEP_ME=kept"), "{seen}");
    for name in [
        "AGENT37_STARTER_TOKEN",
        "OPENCLAW_STARTER_TOKEN",
        "AGENT37_MANAGED_TOKEN",
    ] {
        assert!(!seen.contains(name), "{name} leaked: {seen}");
    }
}
