//! The bypass-mode acceptance written to the Box agent's settings.

use std::fs;

use moor_core::seam::agent_profile::AgentProfile;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{any_interpreter, ctx, forced, install, proxied, read_json, run_step_on, Interp};

fn check_bypass_acceptance(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let key = "skipDangerousModePermissionPrompt";
    for (c, accepted) in [(proxied(t.path()), true), (ctx(t.path()), false)] {
        let settings = format!("{}/.claude/settings.json", c.home.as_str());
        let _ = fs::remove_file(&settings);
        let setup = Claude.hook_writer(&c).unwrap();
        install(&setup);
        assert!(run_step_on(&setup, path).status.success());
        assert_eq!(
            read_json(&settings).get(key),
            accepted.then(|| &json!(true))
        );
        fs::write(&settings, r#"{"model":"x"}"#).unwrap();
        assert!(run_step_on(&setup, path).status.success());
        let merged = read_json(&settings);
        assert_eq!(merged["model"], "x");
        assert_eq!(merged.get(key), accepted.then(|| &json!(true)));
    }
}

#[test]
fn settings_accept_bypass_mode_only_when_launched_in_it() {
    if any_interpreter() {
        check_bypass_acceptance(None);
    }
}

#[test]
fn bypass_acceptance_is_written_with_python3_only() {
    forced(Interp::Python, check_bypass_acceptance);
}

#[test]
fn bypass_acceptance_is_written_with_node_only() {
    forced(Interp::Node, check_bypass_acceptance);
}

#[test]
fn bypass_acceptance_is_written_to_fresh_settings_without_an_interpreter() {
    forced(Interp::Neither, |path| {
        let t = TempDir::new().unwrap();
        let c = proxied(t.path());
        let setup = Claude.hook_writer(&c).unwrap();
        install(&setup);
        assert!(run_step_on(&setup, path).status.success());
        let settings = read_json(&format!("{}/.claude/settings.json", c.home.as_str()));
        assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    });
}
