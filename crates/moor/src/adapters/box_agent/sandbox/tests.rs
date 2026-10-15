use std::process::Command;

use moor_core::name::BoxPath;
use serde_json::Value;

use super::{
    check_failure, check_script, harden_lines, profile, wrap, SandboxInput, NOT_INSTALLED,
};

const SUM: &str = "6da665b61999570c10dffc7da78828fe1fb7f3cac53319e505cebecaef17037f";

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn doc(env_names: &[&str]) -> Value {
    let (w, h, c) = (path("/work/p"), path("/home/u"), path("/tmp/cred"));
    let tmp = path("/home/u/.moor-tmp-t-n");
    let input = SandboxInput {
        box_port: 4321,
        workdir: &w,
        home: &h,
        cred: &c,
        tmp: &tmp,
        env_names: env_names.iter().map(|s| (*s).to_owned()).collect(),
        set_vars: vec![("APP_MODE".to_owned(), "it's".to_owned())],
    };
    serde_json::from_str(&profile(&input)).unwrap()
}

fn at<'a>(v: &'a Value, pointer: &str) -> &'a Value {
    v.pointer(pointer).unwrap_or(&Value::Null)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn profile_blocks_pathname_sockets_and_network() {
    let d = doc(&[]);
    assert_eq!(at(&d, "/linux/af_unix_mediation"), "pathname");
    assert_eq!(at(&d, "/network/block"), &Value::Bool(true));
    assert_eq!(at(&d, "/network/open_port"), &serde_json::json!([4321]));
}

#[test]
fn profile_inlines_the_default_profile_instead_of_extending_it() {
    let d = doc(&[]);
    assert_eq!(at(&d, "/extends"), &Value::Null);
    let groups = strings(at(&d, "/groups/include"));
    assert_eq!(groups.len(), 18);
    for name in [
        "deny_credentials",
        "system_read_linux_core",
        "dangerous_commands_linux",
    ] {
        assert!(groups.contains(&name.to_owned()), "{groups:?}");
    }
    assert_eq!(at(&d, "/security/signal_mode"), "isolated");
    assert_eq!(
        strings(at(&d, "/filesystem/unix_socket_dir")),
        ["/run/systemd/userdb"]
    );
}

#[test]
fn profile_grants_workdir_and_git_config() {
    let d = doc(&[]);
    assert!(strings(at(&d, "/filesystem/allow")).contains(&"/work/p".to_owned()));
    assert!(strings(at(&d, "/filesystem/allow_file")).contains(&"/home/u/.claude.json".to_owned()));
    assert!(strings(at(&d, "/filesystem/read")).contains(&"/tmp/cred".to_owned()));
    let files = strings(at(&d, "/filesystem/read_file"));
    assert!(files.contains(&"/etc/gitconfig".to_owned()));
    assert!(files.contains(&"/home/u/.gitconfig".to_owned()));
}

#[test]
fn env_allow_list_passes_placeholders() {
    let vars = strings(at(
        &doc(&["GH_TOKEN", "GH_REPO", "CLAUDE_CODE_OAUTH_TOKEN"]),
        "/environment/allow_vars",
    ));
    for name in [
        "GH_TOKEN",
        "GH_REPO",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "HTTPS_PROXY",
    ] {
        assert!(vars.contains(&name.to_owned()), "{name}");
    }
    let mut sorted = vars.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(vars, sorted);
}

#[test]
fn env_allow_list_drops_provider_and_gate_names() {
    let vars = strings(at(
        &doc(&["NSC_TOKEN_FILE", "token", "TOKEN", "gate", "run_dir"]),
        "/environment/allow_vars",
    ));
    assert!(!vars.iter().any(|v| v.starts_with("NSC_")));
    for bad in ["token", "TOKEN", "gate", "run_dir"] {
        assert!(!vars.contains(&bad.to_owned()), "{bad}");
    }
}

#[test]
fn profile_grants_the_task_tmp_dir() {
    let d = doc(&[]);
    assert!(strings(at(&d, "/filesystem/allow")).contains(&"/home/u/.moor-tmp-t-n".to_owned()));
}

#[test]
fn profile_never_grants_the_nono_state_root() {
    let d = doc(&[]);
    let state = "/home/u/.local/state";
    for key in ["allow", "allow_file", "read", "read_file"] {
        for p in strings(at(&d, &format!("/filesystem/{key}"))) {
            assert!(
                p != state && !state.starts_with(&format!("{}/", p.trim_end_matches('/'))),
                "{key}: {p}"
            );
        }
    }
    assert!(strings(at(&d, "/filesystem/allow")).contains(&format!("{state}/claude")));
}

#[test]
fn check_script_pins_version_and_checks_sandbox() {
    let s = check_script(SUM);
    assert!(s.contains("= \"nono 0.80.0\""));
    assert!(s.contains(SUM));
    assert!(s.contains("exit 6"));
    assert!(s.contains("setup --check-only"));
    assert!(!s.contains("curl") && !s.contains("sudo"));
}

#[test]
fn check_script_is_valid_sh() {
    let out = Command::new("sh")
        .args(["-n", "-c", &check_script(SUM)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_script_exits_not_installed_without_nono() {
    let home = std::env::temp_dir().join(format!("moor-check-script-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new("sh")
        .args(["-c", &check_script(SUM)])
        .env("HOME", &home)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&home).unwrap();
    assert_eq!(out.status.code(), Some(NOT_INSTALLED));
}

/// Runs the check script with a stub `nono` holding `body` under a fresh home.
fn check_with_stub(tag: &str, body: &[u8], sum: &str) -> Option<i32> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("moor-check-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    let nono = home.join(".local/bin/nono");
    std::fs::write(&nono, body).unwrap();
    std::fs::set_permissions(&nono, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new("sh")
        .args(["-c", &check_script(sum)])
        .env("HOME", &home)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&home).unwrap();
    out.status.code()
}

#[test]
fn check_script_accepts_only_the_pinned_digest() {
    let stub = b"#!/bin/sh\n[ \"$1\" = --version ] && echo 'nono 0.80.0'\nexit 0\n";
    let sum = crate::adapters::box_agent::nono_release::sha256_hex(stub);
    assert_eq!(check_with_stub("same", stub, &sum), Some(0));
    assert_eq!(check_with_stub("other", stub, SUM), Some(NOT_INSTALLED));
}

#[test]
fn wrap_puts_agent_after_separator() {
    let argv = wrap(
        &path("/home/u/.local/bin/nono"),
        &path("/tmp/cred/nono-profile.json"),
        4321,
        &["claude".to_owned(), "-p".to_owned()],
    );
    assert_eq!(
        argv,
        [
            "/home/u/.local/bin/nono",
            "-s",
            "run",
            "--profile",
            "/tmp/cred/nono-profile.json",
            "--block-net",
            "--open-port",
            "4321",
            "--",
            "claude",
            "-p"
        ]
    );
}

#[test]
fn check_failure_names_the_cause() {
    assert!(check_failure(5).contains("Landlock"));
    assert!(check_failure(NOT_INSTALLED).contains("nono 0.80.0 on the Box is missing"));
    assert!(check_failure(9).contains("exited 9"));
}

#[test]
fn set_vars_carry_the_values_byte_exact() {
    let d = doc(&[]);
    assert_eq!(at(&d, "/environment/set_vars/APP_MODE"), "it's");
}

#[test]
fn harden_lines_unset_nono_names_and_pin_the_config_home() {
    let text = harden_lines().join("\n");
    assert!(text.contains("NONO_"), "{text}");
    assert!(text.contains("unset"), "{text}");
    assert!(
        text.contains("export XDG_CONFIG_HOME=\"$HOME/.config\""),
        "{text}"
    );
}
