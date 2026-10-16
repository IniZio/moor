use std::process::Command;

use moor_core::name::BoxPath;
use serde_json::Value;

use super::super::device::{ios::Ios, Device, DeviceKind};
use super::{
    check_failure, check_script, harden_lines, profile, wrap, SandboxInput, NOT_INSTALLED,
};

const LOGIN: &str = "/Users/u";
const TASK_HOME: &str = "/Users/u/.moor-boxes/moor-x/w.moor-home";
const SUM: &str = "6da665b61999570c10dffc7da78828fe1fb7f3cac53319e505cebecaef17037f";

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn doc(env_names: &[&str]) -> Value {
    serde_json::from_str(&text("Linux", env_names)).unwrap()
}

fn text(os: &str, env_names: &[&str]) -> String {
    text_with(os, env_names, &[])
}

fn text_with(os: &str, env_names: &[&str], moor_paths: &[&str]) -> String {
    text_sim(os, env_names, moor_paths, None)
}

fn text_sim(os: &str, env_names: &[&str], moor_paths: &[&str], sim_udid: Option<&str>) -> String {
    let devices: Vec<Device> = sim_udid
        .map(|id| Device {
            kind: &Ios,
            id: id.to_owned(),
        })
        .into_iter()
        .collect();
    text_devices(os, env_names, moor_paths, &devices)
}

fn text_devices(os: &str, env_names: &[&str], moor_paths: &[&str], devices: &[Device]) -> String {
    let moor_paths: Vec<String> = moor_paths.iter().map(|s| (*s).to_owned()).collect();
    let (w, c) = (path("/work/p"), path("/tmp/cred"));
    let (login, h) = if os == "Darwin" {
        (path(LOGIN), path(TASK_HOME))
    } else {
        (path("/home/u"), path("/home/u"))
    };
    let tmp = path("/home/u/.moor-tmp-t-n");
    let input = SandboxInput {
        os,
        workdir: &w,
        home: &h,
        login_home: &login,
        cred: &c,
        tmp: &tmp,
        env_names: env_names.iter().map(|s| (*s).to_owned()).collect(),
        set_vars: vec![("APP_MODE".to_owned(), "it's".to_owned())],
        moor_paths: &moor_paths,
        devices,
    };
    profile(&input)
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
fn darwin_profile_omits_linux_only_keys() {
    let t = text("Darwin", &[]);
    assert!(serde_json::from_str::<Value>(&t)
        .unwrap()
        .get("linux")
        .is_none());
    assert!(!t.contains("/run/systemd"));
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
    assert!(check_failure("Linux", 5).contains("Landlock"));
    assert!(check_failure("Darwin", 5).contains("Seatbelt"));
    assert!(check_failure("Linux", NOT_INSTALLED).contains("nono 0.80.0 on the Box is missing"));
    assert!(check_failure("Linux", 9).contains("exited 9"));
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

fn fs_lists(d: &Value) -> Vec<String> {
    ["allow", "allow_file", "read", "read_file"]
        .iter()
        .flat_map(|k| strings(at(d, &format!("/filesystem/{k}"))))
        .collect()
}

fn darwin() -> Value {
    serde_json::from_str(&text("Darwin", &[])).unwrap()
}

#[test]
fn darwin_profile_never_grants_the_login_users_gh_config() {
    let d = darwin();
    let gh = format!("{LOGIN}/.config/gh");
    for p in fs_lists(&d) {
        assert!(!p.starts_with(&gh), "{p}");
        assert_ne!(p, LOGIN);
    }
    let allow = strings(at(&d, "/filesystem/allow"));
    assert!(
        allow.contains(&format!("{TASK_HOME}/.config/gh")),
        "{allow:?}"
    );
}

#[test]
fn darwin_profile_grants_no_write_under_the_login_home() {
    let d = darwin();
    let mut writable = strings(at(&d, "/filesystem/allow"));
    writable.extend(strings(at(&d, "/filesystem/allow_file")));
    for p in writable {
        assert!(
            !p.starts_with(&format!("{LOGIN}/")) || p.starts_with(TASK_HOME),
            "{p}"
        );
    }
}

#[test]
fn darwin_profile_grants_nothing_in_login_credential_directories() {
    let d = darwin();
    for dir in [".ssh", ".docker", ".orbstack", "Library/Keychains"] {
        let bad = format!("{LOGIN}/{dir}");
        for p in fs_lists(&d) {
            assert!(!p.starts_with(&bad), "{p}");
        }
    }
    let groups = strings(at(&d, "/groups/include"));
    assert!(groups.contains(&"deny_keychains_macos".to_owned()));
    assert!(groups.contains(&"deny_credentials".to_owned()));
}

#[test]
fn darwin_profile_reads_xcode_and_login_toolchains_read_only() {
    let d = darwin();
    let read = strings(at(&d, "/filesystem/read"));
    for p in [
        "/Applications/Xcode.app",
        "/Users/u/Library/Developer/Xcode",
        "/Users/u/.local/share/mise",
        "/Users/u/.local/bin",
    ] {
        assert!(read.contains(&p.to_owned()), "{read:?}");
    }
    assert!(strings(at(&d, "/filesystem/read_file")).contains(&"/Users/u/.gitconfig".to_owned()));
}

#[test]
fn profile_marks_the_environment_as_sandboxed_and_keeps_repo_vars() {
    for os in ["Linux", "Darwin"] {
        let d: Value = serde_json::from_str(&text(os, &[])).unwrap();
        assert_eq!(at(&d, "/environment/set_vars/IS_SANDBOX"), "1");
        assert_eq!(at(&d, "/environment/set_vars/APP_MODE"), "it's");
    }
}

#[test]
fn darwin_profile_blocks_unix_sockets_by_blocking_non_loopback_network() {
    let d = darwin();
    assert_eq!(at(&d, "/network/block"), true);
    assert_eq!(at(&d, "/network/open_port"), &serde_json::json!([0]));
    assert!(doc(&[]).pointer("/network").is_none());
}

fn denies(os: &str, moor_paths: &[&str]) -> Option<Vec<String>> {
    let d: Value = serde_json::from_str(&text_with(os, &[], moor_paths)).unwrap();
    d.pointer("/filesystem/deny").map(strings)
}

#[test]
fn darwin_profile_denies_the_default_moor_paths() {
    let deny = denies("Darwin", &[]).unwrap();
    for rel in [
        ".local/state/moor",
        ".config/moor",
        ".config/agent37",
        ".config/crabbox",
    ] {
        assert!(deny.contains(&format!("{LOGIN}/{rel}")), "{rel}: {deny:?}");
    }
}

#[test]
fn darwin_profile_denies_absolute_moor_paths_once() {
    let deny = denies(
        "Darwin",
        &[
            "/srv/moor-state",
            "/srv/moor-state/",
            "relative/dir",
            "",
            "/Users/u/.config/moor",
        ],
    )
    .unwrap();
    assert_eq!(
        deny.iter().filter(|p| *p == "/srv/moor-state").count(),
        1,
        "{deny:?}"
    );
    assert_eq!(
        deny.iter()
            .filter(|p| *p == "/Users/u/.config/moor")
            .count(),
        1
    );
    assert!(
        !deny.iter().any(|p| p.is_empty() || !p.starts_with('/')),
        "{deny:?}"
    );
    let mut sorted = deny.clone();
    sorted.sort();
    assert_eq!(deny, sorted);
}

#[test]
fn darwin_profile_skips_a_moor_path_that_contains_a_grant() {
    let deny = denies(
        "Darwin",
        &[LOGIN, "/", "/work", "/work/p", "/tmp/cred", "/srv/x"],
    )
    .unwrap();
    for p in [LOGIN, "/", "/work", "/work/p", "/tmp/cred"] {
        assert!(!deny.contains(&p.to_owned()), "{p}: {deny:?}");
    }
    assert!(deny.contains(&"/srv/x".to_owned()));
    assert!(deny.contains(&format!("{LOGIN}/.config/moor")));
}

#[test]
fn linux_profile_has_no_deny_key() {
    assert!(denies("Linux", &["/srv/moor-state"]).is_none());
}

#[test]
fn darwin_profile_grants_nothing_covering_another_tasks_credential_directory() {
    let d = darwin();
    for other in [".moor-cred-other-1", ".moor-tmp-other-1"] {
        let target = format!("{LOGIN}/{other}/proxy-ca.pem");
        for p in fs_lists(&d) {
            assert!(
                target != p && !target.starts_with(&format!("{p}/")),
                "{p} covers {target}"
            );
        }
    }
}

const UDID: &str = "0A1B2C3D-4E5F-6789-ABCD-EF0123456789";
const DEVICES: &str = "/Users/u/Library/Developer/CoreSimulator/Devices";

fn sim(os: &str, udid: Option<&str>) -> Value {
    serde_json::from_str(&text_sim(os, &[], &[], udid)).unwrap()
}

#[test]
fn darwin_profile_grants_exactly_the_tasks_simulator_device_dir_and_sets_its_udid() {
    let d = sim("Darwin", Some(UDID));
    let device = format!("{DEVICES}/{UDID}");
    let allow = strings(at(&d, "/filesystem/allow"));
    assert!(allow.contains(&device), "{allow:?}");
    let under_devices: Vec<_> = fs_lists(&d)
        .into_iter()
        .filter(|p| p.starts_with(DEVICES))
        .collect();
    assert_eq!(under_devices, std::slice::from_ref(&device));
    assert_eq!(at(&d, "/environment/set_vars/MOOR_SIM_UDID"), UDID);
    let deny = strings(at(&d, "/filesystem/deny"));
    assert!(
        !deny.iter().any(|p| device.starts_with(p.as_str())),
        "{deny:?}"
    );
}

#[test]
fn darwin_profile_with_a_simulator_grants_nothing_covering_the_devices_dir_or_a_sibling() {
    let d = sim("Darwin", Some(UDID));
    let sibling = format!("{DEVICES}/11111111-2222-3333-4444-555555555555");
    for p in fs_lists(&d) {
        for target in [DEVICES, sibling.as_str()] {
            assert!(
                p != target && !target.starts_with(&format!("{p}/")),
                "{p} covers {target}"
            );
        }
    }
}

#[test]
fn darwin_profile_ignores_a_malformed_simulator_udid() {
    let plain = sim("Darwin", None);
    for bad in [
        "",
        "../..",
        "0a1b2c3d-4e5f-6789-abcd-ef0123456789",
        "0A1B2C3D-4E5F-6789-ABCD-EF012345678",
        "0A1B2C3D-4E5F-6789-ABCD-EF0123456789/..",
        "GA1B2C3D-4E5F-6789-ABCD-EF0123456789",
    ] {
        let d = sim("Darwin", Some(bad));
        assert_eq!(d, plain, "{bad:?} must grant nothing");
        assert_eq!(at(&d, "/environment/set_vars/MOOR_SIM_UDID"), &Value::Null);
    }
}

#[test]
fn linux_profile_ignores_the_simulator_udid() {
    assert_eq!(sim("Linux", Some(UDID)), sim("Linux", None));
}

/// A device kind hosted on Linux, to prove the profile takes grants and env from any kind.
struct Fake;

impl DeviceKind for Fake {
    fn available(&self, os: &str) -> bool {
        os == "Linux"
    }
    fn provision(
        &self,
        _: &dyn moor_core::seam::driver::Driver,
        _: &moor_core::seam::engine::LaunchReq,
        _: &BoxPath,
    ) -> Option<String> {
        None
    }
    fn env(&self, id: &str) -> Vec<(String, String)> {
        vec![("FAKE_DEVICE".to_owned(), id.to_owned())]
    }
    fn grants(&self, id: &str, login_home: &str) -> Vec<String> {
        vec![format!("{login_home}/.fake/{id}")]
    }
    fn brief(&self, _: &str) -> String {
        String::new()
    }
    fn proof(&self, _: &str) -> String {
        String::new()
    }
    fn teardown(&self, _: &str) -> Option<String> {
        None
    }
    fn release_line(&self, _: &str) -> String {
        String::new()
    }
}

#[test]
fn a_device_kind_available_on_the_os_lands_its_grants_and_env_in_the_profile() {
    let devices = [Device {
        kind: &Fake,
        id: "d1".to_owned(),
    }];
    let d: Value = serde_json::from_str(&text_devices("Linux", &[], &[], &devices)).unwrap();
    let allow = strings(at(&d, "/filesystem/allow"));
    assert!(allow.contains(&"/home/u/.fake/d1".to_owned()), "{allow:?}");
    assert_eq!(at(&d, "/environment/set_vars/FAKE_DEVICE"), "d1");
    let d: Value = serde_json::from_str(&text_devices("Darwin", &[], &[], &devices)).unwrap();
    assert_eq!(at(&d, "/environment/set_vars/FAKE_DEVICE"), &Value::Null);
}
