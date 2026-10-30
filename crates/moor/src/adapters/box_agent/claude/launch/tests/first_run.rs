//! Each first-run screen Claude Code can show is skipped by a setting the start writes, so no
//! one has to answer it on a headless Box. Every check runs under python3, node and plain sh.

use std::fs;

use moor_core::seam::agent_adapter::{AgentAdapter, LaunchCtx};
use serde_json::Value;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced, install, proxied, read_json, run_step_on, Interp};

/// A fresh home after the preseed and hook steps: `~/.claude.json`, then `~/.claude/settings.json`.
fn seeded(c: &LaunchCtx, path: Option<&str>) -> (Value, Value) {
    fs::create_dir_all(c.home.as_str()).unwrap();
    for setup in [Claude.preseed(c).unwrap(), Claude.hook_writer(c).unwrap()] {
        install(&setup);
        assert!(run_step_on(&setup, path).status.success());
    }
    (
        read_json(&format!("{}/.claude.json", c.home.as_str())),
        read_json(&format!("{}/.claude/settings.json", c.home.as_str())),
    )
}

fn on_every_interpreter(check: fn(Option<&str>)) {
    for interp in [Interp::Python, Interp::Node, Interp::Neither] {
        forced(interp, check);
    }
}

/// The onboarding flow holds the theme picker, the security notes and their Enter prompt.
fn onboarding_done(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let (claude, _) = seeded(&ctx(t.path()), path);
    assert_eq!(claude["hasCompletedOnboarding"], true);
}

#[test]
fn theme_is_skipped_by_completed_onboarding_with_a_theme_set() {
    on_every_interpreter(|path| {
        let t = TempDir::new().unwrap();
        let (claude, _) = seeded(&ctx(t.path()), path);
        assert_eq!(claude["hasCompletedOnboarding"], true);
        assert_eq!(claude["theme"], "dark");
    });
}

#[test]
fn security_notes_are_skipped_by_completed_onboarding() {
    on_every_interpreter(onboarding_done);
}

#[test]
fn press_enter_is_skipped_by_completed_onboarding() {
    on_every_interpreter(onboarding_done);
}

#[test]
fn folder_trust_is_skipped_by_has_trust_dialog_accepted() {
    on_every_interpreter(|path| {
        let t = TempDir::new().unwrap();
        let c = ctx(t.path());
        let (claude, _) = seeded(&c, path);
        assert_eq!(
            claude["projects"][c.workdir.as_str()]["hasTrustDialogAccepted"],
            true
        );
    });
}

#[test]
fn bypass_permissions_is_skipped_by_skip_dangerous_mode_permission_prompt() {
    on_every_interpreter(|path| {
        let t = TempDir::new().unwrap();
        let (_, settings) = seeded(&proxied(t.path()), path);
        assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    });
}

#[test]
fn mcp_servers_are_skipped_by_enable_all_project_mcp_servers() {
    on_every_interpreter(|path| {
        let (plain, behind_proxy) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        for c in [ctx(plain.path()), proxied(behind_proxy.path())] {
            let (_, settings) = seeded(&c, path);
            assert_eq!(settings["enableAllProjectMcpServers"], true);
        }
    });
}

#[test]
fn external_imports_are_skipped_by_the_warning_shown_without_approving_them() {
    on_every_interpreter(|path| {
        let t = TempDir::new().unwrap();
        let c = ctx(t.path());
        let (claude, _) = seeded(&c, path);
        let project = &claude["projects"][c.workdir.as_str()];
        assert_eq!(project["hasClaudeMdExternalIncludesWarningShown"], true);
        assert_eq!(project.get("hasClaudeMdExternalIncludesApproved"), None);
    });
}
