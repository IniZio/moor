use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxPath, ProviderId, Secret, TaskId, TaskName};

use super::{github_repo, push_url, script_lines, stage, NONCE_FILE};
use crate::testing::driver::FakeDriver;

const SENTINEL: &[u8] = b"sentinel-9c2e";
const PORT: u16 = 4321;
const CA: &str = "-----BEGIN CERTIFICATE-----\nQUJDREVGRw==\n-----END CERTIFICATE-----\n";

fn proxy() -> ProxyAddr {
    ProxyAddr {
        box_port: PORT,
        nonce: Secret::new(SENTINEL.to_vec()),
        ca_pem: CA.to_owned(),
        box_env: Vec::new(),
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

#[test]
fn github_origins_map_to_one_push_url() {
    let want = Some("https://github.com/o/n.git".to_owned());
    for origin in [
        "https://github.com/o/n",
        "https://github.com/o/n.git",
        "https://github.com/o/n/",
        "ssh://git@github.com/o/n.git",
        "ssh://git@github.com/o/n",
        "git@github.com:o/n.git",
        "git@github.com:o/n",
    ] {
        assert_eq!(push_url(origin), want, "{origin}");
        assert_eq!(
            github_repo(origin),
            Some(ForgeRepo::new("o", "n")),
            "{origin}"
        );
    }
}

#[test]
fn other_origins_are_not_github() {
    for origin in [
        "https://gitlab.com/o/n.git",
        "git@gitlab.com:o/n.git",
        "ssh://git@example.com/o/n.git",
        "/srv/git/o/n.git",
        "https://github.com/o",
        "https://github.com/o/",
        "https://github.com/o/n/extra",
        "https://github.com/o/n/extra.git",
        "",
    ] {
        assert_eq!(push_url(origin), None, "{origin}");
        assert_eq!(github_repo(origin), None, "{origin}");
    }
}

fn lines_for(origin: Option<&str>) -> Vec<String> {
    let mut p = proxy();
    p.box_env = vec![
        ("GH_TOKEN".to_owned(), "moor-proxy".to_owned()),
        ("QUOTED".to_owned(), "it's".to_owned()),
    ];
    script_lines(
        &cred(Path::new("/home/box/.moor-cred")),
        &p,
        &placeholder_env(),
        origin,
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
    assert!(script_lines(&c, &empty, &[], None).is_err());
    let bad = [EnvVar {
        name: "A B".to_owned(),
        value: Secret::new(b"x".to_vec()),
    }];
    assert!(script_lines(&c, &proxy(), &bad, None).is_err());
    let binary = [EnvVar {
        name: "A".to_owned(),
        value: Secret::new(vec![0xff]),
    }];
    assert!(script_lines(&c, &proxy(), &binary, None).is_err());
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
    let proxy_url = format!("http://moor:sentinel-9c2e@127.0.0.1:{PORT}");
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        is(&env, name, &proxy_url);
    }
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

#[test]
fn on_macos_the_system_bundle_is_never_touched_and_no_sudo_runs() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(NONCE_FILE), SENTINEL).unwrap();
    std::fs::write(dir.path().join("proxy-ca.pem"), CA).unwrap();
    let bin = dir.path().join("fakebin");
    std::fs::create_dir(&bin).unwrap();
    let calls = dir.path().join("sudo-calls");
    for (name, body) in [
        ("uname", "#!/bin/sh\necho Darwin\n".to_owned()),
        (
            "sudo",
            format!("#!/bin/sh\necho \"$@\" >>{}\nexit 0\n", calls.display()),
        ),
    ] {
        let tool = bin.join(name);
        std::fs::write(&tool, body).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let system = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/cert.pem",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file());
    let before = system.map(|p| std::fs::read(p).unwrap());
    let lines = script_lines(&cred(dir.path()), &proxy(), &[], None).unwrap();
    assert!(lines.iter().all(|l| !l.contains("sentinel-9c2e")));
    let script_path = dir.path().join("launch.sh");
    std::fs::write(&script_path, format!("{}\nenv", lines.join("\n"))).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = Command::new("sh")
        .arg(&script_path)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!calls.exists(), "sudo was invoked");
    assert_eq!(before, system.map(|p| std::fs::read(p).unwrap()));
    let bundle = dir.path().join("ca-bundle.pem");
    assert_ca_vars(&parse_env(&out.stdout), dir.path(), &bundle);
    let built = std::fs::read(&bundle).unwrap();
    assert!(built.ends_with(CA.as_bytes()));
    if let Some(sys) = before {
        assert!(built.starts_with(&sys));
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
fn stage_puts_the_nonce_and_the_ca_under_the_credential_directory() {
    let d = FakeDriver::new();
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    let c = cred(Path::new("/home/box/.moor-cred"));
    stage(
        &d,
        &BoxHandle::new("box-1"),
        &c,
        &proxy(),
        Path::new("/r/wt"),
    )
    .unwrap();
    let puts = d.puts();
    assert_eq!(puts.len(), 2);
    let mut it = puts.iter();
    let (first, second) = (it.next().unwrap(), it.next().unwrap());
    let (handle, path, bytes, mode, cwd) = first;
    assert_eq!(handle, &BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), "/home/box/.moor-cred/proxy-nonce");
    assert_eq!(bytes, &SENTINEL.to_vec());
    assert_eq!(*mode, 0o600);
    assert_eq!(cwd, Path::new("/r/wt"));
    let (_, path, bytes, mode, _) = second;
    assert_eq!(path.as_str(), "/home/box/.moor-cred/proxy-ca.pem");
    assert_eq!(bytes.as_slice(), CA.as_bytes());
    assert_eq!(*mode, 0o644);
}
