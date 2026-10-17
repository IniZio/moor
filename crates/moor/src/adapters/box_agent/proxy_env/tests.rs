use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::exec_env::{self, BoxCtx, MacTask, ProxyCtx, Purpose, NONCE_FILE};
use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::BoxStep;
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxPath, Secret, TaskName};

use super::{script_lines, stage_steps};

const SENTINEL: &[u8] = b"sentinel-9c2e";
const PORT: u16 = 4321;
const CA: &str = "-----BEGIN CERTIFICATE-----\nQUJDREVGRw==\n-----END CERTIFICATE-----\n";

fn proxy() -> ProxyAddr {
    ProxyAddr {
        box_port: PORT,
        nonce: Secret::new(SENTINEL.to_vec()),
        ca_pem: CA.to_owned(),
        box_env: Vec::new(),
        forge: false,
    }
}

fn placeholder_env() -> Vec<EnvVar> {
    vec![EnvVar {
        name: "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        value: Secret::new(b"moor-proxy".to_vec()),
    }]
}

fn cred(dir: &Path) -> BoxPath {
    BoxPath::restore(dir.to_str().unwrap()).unwrap()
}

fn workdir() -> BoxPath {
    BoxPath::restore("/work").unwrap()
}

#[test]
fn the_agent_gets_the_same_box_environment_as_run() {
    let c = cred(Path::new("/home/box/.moor-cred"));
    let w = workdir();
    let mut p = proxy();
    p.box_env = vec![("QUOTED".to_owned(), "it's".to_owned())];
    let strip = ["PLATFORM_TOKEN".to_owned(), "PLATFORM_TOKEN".to_owned()];
    let origin = Some("git@github.com:o/n.git");
    let agent = super::exec_env(&c, Some(&p), &strip, origin, &w, None).unwrap();
    let run = exec_env::build(
        &BoxCtx {
            env: &p.box_env,
            strip: &strip,
            proxy: Some(ProxyCtx {
                box_port: PORT,
                cred: &c,
            }),
            origin,
            workdir: Some(&w),
            mac: None,
        },
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(agent.prelude, run.prelude);
    assert_eq!(agent.vars, run.vars);
    assert_eq!(agent.unset, ["PLATFORM_TOKEN"]);
    assert_eq!(agent.unset, run.unset);
    assert_eq!(agent.cwd, run.cwd);
    assert_eq!(
        script_lines(&c, &p, &[], origin, None).unwrap(),
        run.prelude
    );
}

#[test]
fn without_a_proxy_the_environment_only_strips_names() {
    let w = workdir();
    let strip = ["PLATFORM_TOKEN".to_owned()];
    let exec = super::exec_env(&w, None, &strip, None, &w, None).unwrap();
    assert_eq!(exec.unset, strip);
    assert!(exec.prelude.is_empty() && exec.vars.is_empty());
}

fn lines_for(origin: Option<&str>) -> Vec<String> {
    let mut p = proxy();
    p.box_env = vec![
        ("GH_TOKEN".to_owned(), "gho_moor-proxy".to_owned()),
        ("QUOTED".to_owned(), "it's".to_owned()),
    ];
    script_lines(
        &cred(Path::new("/home/box/.moor-cred")),
        &p,
        &placeholder_env(),
        origin,
        None,
    )
    .unwrap()
}

#[test]
fn the_lines_never_contain_the_nonce_and_carry_the_exports() {
    let lines = lines_for(Some("git@github.com:o/n.git"));
    let text = lines.join("\n");
    assert!(!text.contains("sentinel-9c2e"), "{text}");
    for want in [
        "moor_nonce=$(cat '/home/box/.moor-cred/proxy-nonce') && rm -f '/home/box/.moor-cred/proxy-nonce' && [ -n \"$moor_nonce\" ] || exit 1",
        "unset GH_TOKEN GITHUB_TOKEN",
        "export SSL_CERT_FILE=\"$moor_bundle\"",
        "export CURL_CA_BUNDLE=\"$moor_bundle\"",
        "export REQUESTS_CA_BUNDLE=\"$moor_bundle\"",
        "export GIT_SSL_CAINFO=\"$moor_bundle\"",
        "export NODE_EXTRA_CA_CERTS='/home/box/.moor-cred/proxy-ca.pem'",
        "export HTTPS_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export HTTP_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export https_proxy=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export http_proxy=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export YARN_HTTPS_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export YARN_HTTP_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export YARN_CACHE_FOLDER=\"$HOME/.cache/yarn\"",
        "export YARN_GLOBAL_FOLDER=\"$HOME/.cache/yarn/berry\"",
        "export npm_config_cache=\"$HOME/.cache/npm\"",
        "export NODE_USE_ENV_PROXY=1",
        "export CLAUDE_CODE_OAUTH_TOKEN='moor-proxy'",
        "export GIT_CONFIG_COUNT=2",
        "export GIT_CONFIG_KEY_0=remote.moor.url",
        "export GIT_CONFIG_VALUE_0='https://github.com/o/n.git'",
        "export GIT_CONFIG_KEY_1=remote.moor.fetch",
        "export GIT_CONFIG_VALUE_1='+refs/heads/*:refs/remotes/moor/*'",
        "unset moor_nonce moor_bundle moor_sys moor_mark moor_f",
    ] {
        assert!(lines.iter().any(|l| l == want), "missing {want}\n{text}");
    }
    for gone in ["insteadOf", "/t/", "MOOR_"] {
        assert!(!text.contains(gone), "{gone}\n{text}");
    }
}

#[test]
fn no_origin_unsets_the_git_config() {
    let text = lines_for(None).join("\n");
    assert!(text.contains("unset GIT_CONFIG_COUNT"), "{text}");
    assert!(!text.contains("GIT_CONFIG_KEY"), "{text}");
}

#[test]
fn the_profile_env_comes_after_the_proxy_env() {
    let lines = lines_for(None);
    let at = |w: &str| lines.iter().position(|l| l.contains(w)).unwrap();
    assert!(at("NODE_USE_ENV_PROXY") < at("CLAUDE_CODE_OAUTH_TOKEN"));
}

#[test]
fn rule_and_repo_env_is_left_to_the_sandbox_profile() {
    let text = lines_for(None).join("\n");
    assert!(!text.contains("QUOTED"), "{text}");
    assert!(!text.contains("export GH_TOKEN="), "{text}");
}

#[test]
fn an_empty_nonce_or_bad_name_is_refused() {
    let c = cred(Path::new("/c"));
    let empty = ProxyAddr {
        nonce: Secret::new(Vec::new()),
        ..proxy()
    };
    assert!(script_lines(&c, &empty, &[], None, None).is_err());
    let bad = [EnvVar {
        name: "A B".to_owned(),
        value: Secret::new(b"x".to_vec()),
    }];
    assert!(script_lines(&c, &proxy(), &bad, None, None).is_err());
    let binary = [EnvVar {
        name: "A".to_owned(),
        value: Secret::new(vec![0xff]),
    }];
    assert!(script_lines(&c, &proxy(), &binary, None, None).is_err());
}

#[test]
fn the_script_runs_and_builds_the_environment_from_the_staged_nonce() {
    let dir = tempfile::TempDir::new().unwrap();
    let nonce_file = dir.path().join(NONCE_FILE);
    std::fs::write(&nonce_file, SENTINEL).unwrap();
    std::fs::write(dir.path().join("proxy-ca.pem"), CA).unwrap();
    let fake = fake_sudo(dir.path());
    let lines = script_lines(
        &cred(dir.path()),
        &proxy(),
        &placeholder_env(),
        Some("https://github.com/o/n"),
        None,
    )
    .unwrap();
    let script = format!("{}\nenv", lines.join("\n"));
    let script_path = dir.path().join("launch.sh");
    std::fs::write(&script_path, &script).unwrap();
    let syntax = Command::new("sh")
        .arg("-n")
        .arg(&script_path)
        .output()
        .unwrap();
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
    let path = format!("{}:{}", fake.display(), std::env::var("PATH").unwrap());
    let out = Command::new("sh")
        .arg(&script_path)
        .env("PATH", path)
        .env("GH_TOKEN", "real-gh")
        .env("GITHUB_TOKEN", "real-github")
        .env("f", "keep")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = parse_env(&out.stdout);
    assert_yarn_vars(&env);
    assert_ca_vars(&env, dir.path(), &dir.path().join("ca-bundle.pem"));
    is(&env, "CLAUDE_CODE_OAUTH_TOKEN", "moor-proxy");
    is(&env, "GIT_CONFIG_VALUE_0", "https://github.com/o/n.git");
    is(&env, "NODE_USE_ENV_PROXY", "1");
    is(&env, "f", "keep");
    assert!(std::fs::read_to_string(dir.path().join("ca-bundle.pem"))
        .unwrap()
        .ends_with(CA));
    assert!(!nonce_file.exists());
    for gone in ["GH_TOKEN", "GITHUB_TOKEN", "moor_nonce", "moor_bundle"] {
        assert!(!env.contains_key(gone), "{gone}");
    }
    for name in ["NO_PROXY", "no_proxy"] {
        let v = env.get(name).map_or("", String::as_str);
        assert!(v.contains("127.0.0.1") && v.contains("localhost"), "{name}");
    }
}

/// A directory holding a `sudo` that always refuses.
fn fake_sudo(dir: &Path) -> PathBuf {
    let fake = dir.join("fakebin");
    std::fs::create_dir(&fake).unwrap();
    let sudo = fake.join("sudo");
    std::fs::write(&sudo, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake
}

/// The `NAME=value` lines of `env` output as a map.
fn parse_env(stdout: &[u8]) -> HashMap<String, String> {
    String::from_utf8(stdout.to_vec())
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Asserts every CA bundle variable points at the built bundle and Node at the proxy CA.
fn assert_ca_vars(env: &HashMap<String, String>, dir: &Path, bundle: &Path) {
    let bundle_str = bundle.to_str().unwrap();
    for name in [
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "REQUESTS_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        is(env, name, bundle_str);
    }
    is(
        env,
        "NODE_EXTRA_CA_CERTS",
        dir.join("proxy-ca.pem").to_str().unwrap(),
    );
}

/// Asserts the variable `k` is `v`.
fn is(env: &HashMap<String, String>, k: &str, v: &str) {
    assert_eq!(env.get(k).map(String::as_str), Some(v), "{k}");
}

#[test]
fn stage_steps_put_the_nonce_and_the_ca_under_the_credential_directory() {
    let c = cred(Path::new("/home/box/.moor-cred"));
    let [(nonce, nonce_text), (ca, ca_text)] = stage_steps(&c, &proxy()).unwrap();
    let put = |path: &str, bytes: Vec<u8>, mode| BoxStep::Put {
        path: BoxPath::restore(path).unwrap(),
        bytes,
        mode,
    };
    assert_eq!(
        nonce,
        put("/home/box/.moor-cred/proxy-nonce", SENTINEL.to_vec(), 0o600)
    );
    assert_eq!(nonce_text, "could not stage the proxy nonce");
    assert_eq!(
        ca,
        put(
            "/home/box/.moor-cred/proxy-ca.pem",
            CA.as_bytes().to_vec(),
            0o644
        )
    );
    assert_eq!(ca_text, "could not stage the proxy CA");
}

/// Asserts the proxy URL reaches the usual and Yarn variables, and Yarn gets the CA bundle and a cache under `$HOME/.cache`.
fn assert_yarn_vars(env: &HashMap<String, String>) {
    let proxy_url = format!("http://moor:sentinel-9c2e@127.0.0.1:{PORT}");
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        is(env, name, &proxy_url);
    }
    is(env, "YARN_HTTPS_PROXY", &proxy_url);
    is(env, "YARN_HTTP_PROXY", &proxy_url);
    assert!(!env.contains_key("YARN_HTTPS_CA_FILE_PATH"));
    assert!(!env.contains_key("YARN_CA_FILE_PATH"));
    let home = std::env::var("HOME").unwrap();
    is(env, "YARN_CACHE_FOLDER", &format!("{home}/.cache/yarn"));
    is(
        env,
        "YARN_GLOBAL_FOLDER",
        &format!("{home}/.cache/yarn/berry"),
    );
    is(env, "npm_config_cache", &format!("{home}/.cache/npm"));
}

fn mac_layout() -> (MacLayout, TaskKey) {
    let key = TaskKey::new(&TaskName::parse("fix-login").unwrap());
    let layout = MacLayout::new(&BoxPath::restore("/Users/Shared/moor").unwrap(), &key).unwrap();
    (layout, key)
}

#[test]
fn a_mac_task_gives_the_agent_its_home_and_box_tag() {
    let (layout, key) = mac_layout();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "n1",
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: None,
    };
    let c = cred(Path::new("/home/box/.moor-cred"));
    let w = workdir();
    let exec = super::exec_env(&c, Some(&proxy()), &[], None, &w, Some(mac)).unwrap();
    let get = |k: &str| {
        exec.vars
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("HOME"), Some(layout.home.as_str()));
    assert_eq!(
        get("MOOR_BOX_TAG"),
        Some(format!("{}-n1", key.as_str()).as_str())
    );
}

#[test]
fn a_mac_task_exports_no_home_before_the_sandbox_starts() {
    let (layout, key) = mac_layout();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "n1",
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: None,
    };
    let c = cred(Path::new("/home/box/.moor-cred"));
    let lines = script_lines(&c, &proxy(), &placeholder_env(), None, Some(mac)).unwrap();
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "CFFIXED_USER_HOME",
    ] {
        let prefix = format!("export {name}=");
        assert!(!lines.iter().any(|l| l.starts_with(&prefix)), "{lines:?}");
    }
    assert!(
        !lines.iter().any(|l| l.contains(layout.home.as_str())),
        "{lines:?}"
    );
    let token = "export CLAUDE_CODE_OAUTH_TOKEN='moor-proxy'";
    assert!(lines.iter().any(|l| l == token), "{lines:?}");
}
