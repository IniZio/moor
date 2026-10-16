use std::process::Command;

use moor_core::name::BoxPath;
use serde_json::Value;

use super::{
    check_failure, check_script, harden_lines, namespace_lines, profile, wrap, SandboxInput,
    NOT_INSTALLED,
};

const SUM: &str = "6da665b61999570c10dffc7da78828fe1fb7f3cac53319e505cebecaef17037f";

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn doc(env_names: &[&str]) -> Value {
    doc_as(env_names, false)
}

fn doc_as(env_names: &[&str], root: bool) -> Value {
    let (w, h, c) = (path("/work/p"), path("/home/u"), path("/tmp/cred"));
    let tmp = path("/home/u/.moor-tmp-t-n");
    let input = SandboxInput {
        workdir: &w,
        home: &h,
        cred: &c,
        tmp: &tmp,
        env_names: env_names.iter().map(|s| (*s).to_owned()).collect(),
        set_vars: vec![("APP_MODE".to_owned(), "it's".to_owned())],
        root,
        strip_env: &[],
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
fn profile_blocks_pathname_sockets_and_leaves_the_network_open() {
    let d = doc(&[]);
    assert_eq!(at(&d, "/linux/af_unix_mediation"), "pathname");
    assert_eq!(at(&d, "/network"), &Value::Null);
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

fn doc_with_set_vars(
    env_names: &[&str],
    set_vars: Vec<(String, String)>,
    strip_env: &[String],
) -> Value {
    let (w, h, c) = (path("/work/p"), path("/home/u"), path("/tmp/cred"));
    let tmp = path("/home/u/.moor-tmp-t-n");
    let input = SandboxInput {
        workdir: &w,
        home: &h,
        cred: &c,
        tmp: &tmp,
        env_names: env_names.iter().map(|s| (*s).to_owned()).collect(),
        set_vars,
        root: false,
        strip_env,
    };
    serde_json::from_str(&profile(&input)).unwrap()
}

#[test]
fn stripped_names_never_reach_the_sandbox_environment() {
    let strip = ["PLATFORM_TOKEN".to_owned()];
    let set_vars = vec![
        ("PLATFORM_TOKEN".to_owned(), "secret".to_owned()),
        ("APP_MODE".to_owned(), "x".to_owned()),
    ];
    let d = doc_with_set_vars(&["PLATFORM_TOKEN", "APP_ENV"], set_vars, &strip);
    let allowed = strings(at(&d, "/environment/allow_vars"));
    let set = at(&d, "/environment/set_vars").as_object().unwrap();
    assert!(
        !allowed.contains(&"PLATFORM_TOKEN".to_owned()),
        "allow_vars"
    );
    assert!(allowed.contains(&"APP_ENV".to_owned()));
    assert!(!set.contains_key("PLATFORM_TOKEN"), "set_vars");
    assert!(set.contains_key("APP_MODE"));
}

#[test]
fn stripped_names_are_dropped_from_set_vars_alone() {
    let strip = ["PLATFORM_TOKEN".to_owned()];
    let set_vars = vec![("PLATFORM_TOKEN".to_owned(), "secret".to_owned())];
    let d = doc_with_set_vars(&[], set_vars, &strip);
    let set = at(&d, "/environment/set_vars").as_object().unwrap();
    assert!(!set.contains_key("PLATFORM_TOKEN"));
}

#[test]
fn names_pass_when_strip_env_is_empty() {
    let set_vars = vec![("PLATFORM_TOKEN".to_owned(), "v".to_owned())];
    let d = doc_with_set_vars(&["PLATFORM_TOKEN"], set_vars, &[]);
    assert!(strings(at(&d, "/environment/allow_vars")).contains(&"PLATFORM_TOKEN".to_owned()));
    assert!(at(&d, "/environment/set_vars")
        .as_object()
        .unwrap()
        .contains_key("PLATFORM_TOKEN"));
}

#[test]
fn profile_never_grants_proc_run_or_root() {
    let d = doc(&[]);
    let fs = at(&d, "/filesystem").as_object().unwrap();
    let mut seen = 0;
    for (key, value) in fs {
        let Some(list) = value.as_array() else {
            continue;
        };
        for p in strings(&Value::Array(list.clone())) {
            seen += 1;
            let p = p.trim_end_matches('/');
            assert!(!p.is_empty(), "{key}: grants /");
            let exposed = ["/proc", "/run"]
                .iter()
                .any(|r| p == *r || p.starts_with(&format!("{r}/")));
            let userdb = key == "unix_socket_dir" && p == "/run/systemd/userdb";
            assert!(!exposed || userdb, "{key}: {p}");
        }
    }
    assert!(seen > 5);
}

#[test]
fn profile_groups_stay_within_the_pinned_set() {
    let groups = strings(at(&doc(&[]), "/groups/include"));
    let pinned = [
        "deny_credentials",
        "deny_keychains_macos",
        "deny_keychains_linux",
        "deny_browser_data_macos",
        "deny_browser_data_linux",
        "deny_macos_private",
        "deny_shell_history",
        "deny_shell_configs",
        "system_read_macos",
        "system_read_linux_core",
        "system_write_macos",
        "system_write_linux",
        "user_tools",
        "homebrew_macos",
        "homebrew_linux",
        "dangerous_commands",
        "dangerous_commands_macos",
        "dangerous_commands_linux",
    ];
    for g in &groups {
        assert!(pinned.contains(&g.as_str()), "unpinned group {g}");
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

#[test]
fn profile_tells_claude_it_is_sandboxed_only_for_root() {
    let var = "/environment/set_vars/IS_SANDBOX";
    assert_eq!(at(&doc_as(&[], true), var), "1");
    assert_eq!(at(&doc_as(&[], false), var), &Value::Null);
}

/// Which `unshare` the Box has.
enum Unshare {
    Missing,
    Old,
    Current,
}

/// Runs the namespace lines as a user whose `id -u` is `uid`, with the `unshare` of kind `unshare`;
/// returns the exit code, stderr and the `moor_ns` value the script ends with.
fn namespace_with(tag: &str, uid: &str, unshare: &Unshare) -> (Option<i32>, String, String) {
    use std::os::unix::fs::PermissionsExt;
    let bin = std::env::temp_dir().join(format!("moor-ns-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    let stub = |name: &str, body: &str| {
        let file = bin.join(name);
        std::fs::write(&file, body).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    stub("id", &format!("#!/bin/sh\necho {uid}\n"));
    let help = match *unshare {
        Unshare::Missing => None,
        Unshare::Old => Some("  --pid"),
        Unshare::Current => Some("  --kill-child"),
    };
    if let Some(help) = help {
        stub("unshare", &format!("#!/bin/sh\necho '{help}'\n"));
    }
    std::os::unix::fs::symlink("/usr/bin/grep", bin.join("grep")).unwrap();
    let script = format!("{}\nprintf '%s' \"$moor_ns\"", namespace_lines().join("\n"));
    let out = Command::new("/bin/sh")
        .args(["-c", &script])
        .env("PATH", &bin)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&bin).unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn root_gets_its_own_pid_namespace_with_a_private_proc() {
    let (code, _, prefix) = namespace_with("root", "0", &Unshare::Current);
    assert_eq!(code, Some(0));
    assert_eq!(prefix, "unshare --pid --fork --kill-child --mount-proc --");
}

#[test]
fn a_non_root_user_runs_without_a_namespace() {
    let (code, _, prefix) = namespace_with("user", "1000", &Unshare::Current);
    assert_eq!(code, Some(0));
    assert_eq!(prefix, "");
    let (code, _, prefix) = namespace_with("user-bare", "1000", &Unshare::Missing);
    assert_eq!((code, prefix.as_str()), (Some(0), ""));
}

#[test]
fn root_without_unshare_is_refused() {
    let (code, stderr, prefix) = namespace_with("bare", "0", &Unshare::Missing);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("needs unshare"), "{stderr}");
    assert_eq!(prefix, "");
}

#[test]
fn root_with_an_unshare_lacking_kill_child_is_refused() {
    let (code, stderr, prefix) = namespace_with("old", "0", &Unshare::Old);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("--kill-child"), "{stderr}");
    assert_eq!(prefix, "");
}
