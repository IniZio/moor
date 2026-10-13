use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::{AcquireReq, Driver, EgressPlan};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxPath, ProviderId, Secret, TaskId, TaskName};

use super::{github_repo, push_url, script_lines, stage, NONCE_FILE};
use crate::testing::driver::FakeDriver;

const SENTINEL: &[u8] = b"sentinel-9c2e";
const PORT: u16 = 4321;

fn proxy() -> ProxyAddr {
    ProxyAddr {
        box_port: PORT,
        nonce: Secret::new(SENTINEL.to_vec()),
    }
}

fn claude_env() -> Vec<EnvVar> {
    vec![
        EnvVar {
            name: "ANTHROPIC_BASE_URL".to_owned(),
            value: Secret::new(format!("http://127.0.0.1:{PORT}/t/sentinel-9c2e").into_bytes()),
        },
        EnvVar {
            name: "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            value: Secret::new(SENTINEL.to_vec()),
        },
    ]
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

#[test]
fn the_lines_never_contain_the_nonce_and_carry_the_exports() {
    let lines = script_lines(
        &cred(Path::new("/home/box/.moor-cred")),
        &proxy(),
        &claude_env(),
        Some("git@github.com:o/n.git"),
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(!text.contains("sentinel-9c2e"), "{text}");
    for want in [
        "moor_nonce=$(cat '/home/box/.moor-cred/proxy-nonce') && rm -f '/home/box/.moor-cred/proxy-nonce' && [ -n \"$moor_nonce\" ] || exit 1",
        "unset GH_TOKEN GITHUB_TOKEN",
        "export ANTHROPIC_BASE_URL='http://127.0.0.1:4321/t/'\"$moor_nonce\"",
        "export CLAUDE_CODE_OAUTH_TOKEN=\"$moor_nonce\"",
        "export GIT_CONFIG_COUNT=1",
        "export GIT_CONFIG_KEY_0='url.http://127.0.0.1:4321/t/'\"$moor_nonce\"'/github/.insteadOf'",
        "export GIT_CONFIG_VALUE_0=https://github.com/",
        "export MOOR_GH_API='http://127.0.0.1:4321/t/'\"$moor_nonce\"'/github-api'",
        "export MOOR_PUSH_URL='https://github.com/o/n.git'",
        "export MOOR_GH_REPO='o/n'",
        "export PATH='/home/box/.moor-cred/bin':\"$PATH\"",
        "unset moor_nonce",
    ] {
        assert!(lines.iter().any(|l| l == want), "missing {want}\n{text}");
    }
}

#[test]
fn an_empty_nonce_or_bad_name_is_refused() {
    let c = cred(Path::new("/c"));
    let empty = ProxyAddr {
        box_port: PORT,
        nonce: Secret::new(Vec::new()),
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

fn run(origin: Option<&str>) -> (HashMap<String, String>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let nonce_file = dir.path().join(NONCE_FILE);
    std::fs::write(&nonce_file, SENTINEL).unwrap();
    let lines = script_lines(&cred(dir.path()), &proxy(), &claude_env(), origin).unwrap();
    let script = format!("{}\nenv", lines.join("\n"));
    let out = Command::new("sh")
        .args(["-c", &script])
        .env("GH_TOKEN", "real-gh")
        .env("GITHUB_TOKEN", "real-github")
        .env("MOOR_PUSH_URL", "stale")
        .env("MOOR_GH_REPO", "stale")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    assert!(!nonce_file.exists());
    (env, dir)
}

/// Asserts the variable `k` is `v`.
fn is(env: &HashMap<String, String>, k: &str, v: &str) {
    assert_eq!(env.get(k).map(String::as_str), Some(v), "{k}");
}

#[test]
fn the_shell_builds_the_proxy_variables_from_the_staged_nonce() {
    let (env, _dir) = run(Some("https://github.com/o/n"));
    let base = format!("http://127.0.0.1:{PORT}/t/sentinel-9c2e");
    is(&env, "ANTHROPIC_BASE_URL", &base);
    is(&env, "CLAUDE_CODE_OAUTH_TOKEN", "sentinel-9c2e");
    is(&env, "GIT_CONFIG_COUNT", "1");
    is(
        &env,
        "GIT_CONFIG_KEY_0",
        &format!("url.{base}/github/.insteadOf"),
    );
    is(&env, "GIT_CONFIG_VALUE_0", "https://github.com/");
    is(&env, "MOOR_GH_API", &format!("{base}/github-api"));
    is(&env, "MOOR_PUSH_URL", "https://github.com/o/n.git");
    is(&env, "MOOR_GH_REPO", "o/n");
}

#[test]
fn the_shell_scrubs_credentials_and_puts_the_shim_first_on_path() {
    let (env, dir) = run(Some("https://github.com/o/n"));
    for name in ["NO_PROXY", "no_proxy"] {
        let v = env.get(name).map_or("", String::as_str);
        assert!(v.contains("127.0.0.1") && v.contains("localhost"), "{name}");
    }
    let bin = format!("{}/bin:", dir.path().display());
    let path = env.get("PATH").map_or("", String::as_str);
    assert!(path.starts_with(&bin), "{path}");
    for gone in ["GH_TOKEN", "GITHUB_TOKEN", "moor_nonce"] {
        assert!(!env.contains_key(gone), "{gone}");
    }
}

#[test]
fn no_origin_leaves_the_push_url_and_repo_unset() {
    let (env, _dir) = run(None);
    assert!(!env.contains_key("MOOR_PUSH_URL"));
    assert!(!env.contains_key("MOOR_GH_REPO"));
}

#[test]
fn stage_puts_the_nonce_and_the_shim_under_the_credential_directory() {
    let d = FakeDriver::new();
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        egress: EgressPlan::Open,
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
    let mut puts = d.puts().into_iter();
    let (handle, path, bytes, mode, cwd) = puts.next().unwrap();
    assert_eq!(handle, BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), "/home/box/.moor-cred/proxy-nonce");
    assert_eq!(bytes, SENTINEL.to_vec());
    assert_eq!(mode, 0o600);
    assert_eq!(cwd, Path::new("/r/wt"));
    let (_, path, bytes, mode, _) = puts.next().unwrap();
    assert_eq!(path.as_str(), "/home/box/.moor-cred/bin/gh");
    assert_eq!(
        bytes.as_slice(),
        crate::adapters::box_agent::gh_shim::gh_shim().0
    );
    assert_eq!(mode, 0o755);
}
