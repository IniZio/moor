use std::process::Command;

use moor_core::name::BoxPath;
use serde_json::Value;

use super::{install_failure, install_script, profile, wrap, SandboxInput, ASSETS};

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn doc(env_names: &[&str]) -> Value {
    let (w, h, c) = (path("/work/p"), path("/home/u"), path("/tmp/cred"));
    let input = SandboxInput {
        box_port: 4321,
        workdir: &w,
        home: &h,
        cred: &c,
        env_names: env_names.iter().map(|s| (*s).to_owned()).collect(),
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
    assert_eq!(at(&d, "/extends"), "default");
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
fn install_script_pins_version_and_checksums() {
    let s = install_script();
    assert!(s.contains("!= \"nono 0.80.0\""));
    for (_, _, asset, sum) in ASSETS {
        assert!(s.contains(asset) && s.contains(sum), "{asset}");
    }
    assert!(s.contains("sha256sum -c"));
    assert!(s.contains("setup --check-only"));
    assert!(!s.contains("sudo"));
}

#[test]
fn install_script_is_valid_sh() {
    let out = Command::new("sh")
        .args(["-n", "-c", &install_script()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn wrap_puts_agent_after_separator() {
    let argv = wrap(
        &path("/tmp/cred/nono-profile.json"),
        4321,
        &["claude".to_owned(), "-p".to_owned()],
    );
    assert_eq!(
        argv,
        [
            "nono",
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
fn install_failure_names_the_cause() {
    assert!(install_failure(5).contains("Landlock"));
    assert!(install_failure(3).contains("sha256"));
    assert!(install_failure(9).contains("exited 9"));
}
