use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::exec_env::{self, BoxCtx, ProxyCtx, Purpose};
use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::{BoxPath, Secret};

use super::script_lines;

const PORT: u16 = 4321;
const CA: &str = "-----BEGIN CERTIFICATE-----\nQUJDREVGRw==\n-----END CERTIFICATE-----\n";

fn proxy() -> ProxyAddr {
    ProxyAddr {
        box_port: PORT,
        ca_pem: CA.to_owned(),
        box_env: Vec::new(),
        forge: false,
        direct: Vec::new(),
    }
}

fn placeholder_env() -> Vec<EnvVar> {
    vec![EnvVar {
        name: "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        value: Secret::new(b"moor-proxy".to_vec()),
    }]
}

fn workdir() -> BoxPath {
    BoxPath::restore("/work").unwrap()
}

#[test]
fn the_agent_gets_the_same_box_environment_as_run() {
    let w = workdir();
    let mut p = proxy();
    p.box_env = vec![("QUOTED".to_owned(), "it's".to_owned())];
    let strip = ["PLATFORM_TOKEN".to_owned(), "PLATFORM_TOKEN".to_owned()];
    let origin = Some("git@github.com:o/n.git");
    let agent = super::exec_env(Some(&p), &strip, origin, &w).unwrap();
    let run = exec_env::build(
        &BoxCtx {
            env: &p.box_env,
            strip: &strip,
            proxy: Some(ProxyCtx {
                box_port: PORT,
                ca_pem: &p.ca_pem,
                direct: &[],
            }),
            origin,
            workdir: Some(&w),
        },
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(agent.prelude, run.prelude);
    assert_eq!(agent.vars, run.vars);
    assert_eq!(agent.unset, ["PLATFORM_TOKEN"]);
    assert_eq!(agent.unset, run.unset);
    assert_eq!(agent.cwd, run.cwd);
    assert_eq!(script_lines(&p, &[], origin).unwrap(), run.prelude);
}

#[test]
fn without_a_proxy_the_environment_only_strips_names() {
    let w = workdir();
    let strip = ["PLATFORM_TOKEN".to_owned()];
    let exec = super::exec_env(None, &strip, None, &w).unwrap();
    assert_eq!(exec.unset, strip);
    assert!(exec.vars.is_empty());
    assert!(exec.prelude.iter().all(|l| !l.contains("PROXY")));
}

fn lines_for(origin: Option<&str>) -> Vec<String> {
    let mut p = proxy();
    p.box_env = vec![
        ("GH_TOKEN".to_owned(), "gho_moor-proxy".to_owned()),
        ("QUOTED".to_owned(), "it's".to_owned()),
    ];
    script_lines(&p, &placeholder_env(), origin).unwrap()
}

#[test]
fn the_lines_carry_the_exports() {
    let lines = lines_for(Some("git@github.com:o/n.git"));
    let text = lines.join("\n");
    for want in [
        "unset GH_TOKEN GITHUB_TOKEN",
        "export SSL_CERT_FILE=\"$moor_bundle\"",
        "export CURL_CA_BUNDLE=\"$moor_bundle\"",
        "export REQUESTS_CA_BUNDLE=\"$moor_bundle\"",
        "export GIT_SSL_CAINFO=\"$moor_bundle\"",
        "export NODE_EXTRA_CA_CERTS=\"$HOME/.moor-proxy/ca.pem\"",
        "export HTTPS_PROXY=\"http://127.0.0.1:4321\"",
        "export HTTP_PROXY=\"http://127.0.0.1:4321\"",
        "export https_proxy=\"http://127.0.0.1:4321\"",
        "export http_proxy=\"http://127.0.0.1:4321\"",
        "export YARN_HTTPS_PROXY=\"http://127.0.0.1:4321\"",
        "export YARN_HTTP_PROXY=\"http://127.0.0.1:4321\"",
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
        "unset moor_bundle moor_sys moor_mark moor_f moor_g moor_p moor_ts moor_jh",
    ] {
        assert!(lines.iter().any(|l| l == want), "missing {want}\n{text}");
    }
    assert!(text.contains("export JAVA_TOOL_OPTIONS="), "{text}");
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
fn rule_and_repo_env_is_left_to_the_launch_script() {
    let text = lines_for(None).join("\n");
    assert!(!text.contains("QUOTED"), "{text}");
    assert!(!text.contains("export GH_TOKEN="), "{text}");
}

#[test]
fn a_bad_name_or_value_is_refused() {
    let bad = [EnvVar {
        name: "A B".to_owned(),
        value: Secret::new(b"x".to_vec()),
    }];
    assert!(script_lines(&proxy(), &bad, None).is_err());
    let binary = [EnvVar {
        name: "A".to_owned(),
        value: Secret::new(vec![0xff]),
    }];
    assert!(script_lines(&proxy(), &binary, None).is_err());
}

#[test]
fn the_script_runs_and_builds_the_environment() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let fake = fake_sudo(dir.path());
    let lines = script_lines(&proxy(), &placeholder_env(), Some("https://github.com/o/n")).unwrap();
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
        .env("HOME", &home)
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
    assert_yarn_vars(&env, &home);
    assert_ca_vars(&env, &home);
    is(&env, "CLAUDE_CODE_OAUTH_TOKEN", "moor-proxy");
    is(&env, "GIT_CONFIG_VALUE_0", "https://github.com/o/n.git");
    is(&env, "NODE_USE_ENV_PROXY", "1");
    is(&env, "f", "keep");
    assert!(
        std::fs::read_to_string(home.join(".moor-proxy/ca-bundle.pem"))
            .unwrap()
            .ends_with(CA)
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".moor-proxy/ca.pem")).unwrap(),
        CA
    );
    for gone in ["GH_TOKEN", "GITHUB_TOKEN", "moor_bundle"] {
        assert!(!env.contains_key(gone), "{gone}");
    }
    for name in ["NO_PROXY", "no_proxy"] {
        let v = env.get(name).map_or("", String::as_str);
        assert!(v.contains("127.0.0.1") && v.contains("localhost"), "{name}");
    }
}

/// The result of running the whole launch script in a real `sh` with a controlled PATH.
struct Launch {
    dir: tempfile::TempDir,
    env: HashMap<String, String>,
    keytool_log: PathBuf,
}

/// Host tools the launch script may call, linked into the test PATH so a host JDK never leaks in.
const HOST_TOOLS: [&str; 25] = [
    "sh", "cat", "rm", "mkdir", "chmod", "grep", "sed", "awk", "mv", "cp", "mktemp", "cut", "tr",
    "head", "tail", "env", "id", "dirname", "basename", "touch", "ls", "sort", "ln", "wc", "cmp",
];

fn which(tool: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

/// Runs the launch script with `HOME`/`GRADLE_USER_HOME` under a temp dir and a PATH of fakes plus
/// host tools only; a fake `keytool` is present only when `with_keytool`.
fn run_launch(with_keytool: bool) -> Launch {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = fake_sudo(dir.path());
    for tool in HOST_TOOLS {
        if let Some(src) = which(tool) {
            std::os::unix::fs::symlink(src, bin.join(tool)).unwrap();
        }
    }
    let keytool_log = dir.path().join("keytool.log");
    if with_keytool {
        let keytool = bin.join("keytool");
        let body = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -keystore|-destkeystore) : > \"$2\";; esac\n  shift\ndone\n",
            keytool_log.display()
        );
        moor_testkit::write_executable(&keytool, body).unwrap();
    }
    let home = dir.path().join("home");
    let gradle = dir.path().join("gradle");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&gradle).unwrap();
    std::fs::write(
        gradle.join("gradle.properties"),
        "org.gradle.jvmargs=-Xmx2560m\n",
    )
    .unwrap();
    let lines = script_lines(&proxy(), &placeholder_env(), None).unwrap();
    let script = format!("{}\nenv", lines.join("\n"));
    let script_path = dir.path().join("launch.sh");
    std::fs::write(&script_path, &script).unwrap();
    let out = Command::new("sh")
        .arg(&script_path)
        .env_clear()
        .env("PATH", &bin)
        .env("HOME", &home)
        .env("GRADLE_USER_HOME", &gradle)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Launch {
        env: parse_env(&out.stdout),
        dir,
        keytool_log,
    }
}

#[test]
fn java_builds_get_the_proxy_and_gradle_holds_no_proxy_login() {
    let l = run_launch(true);
    let opts = l.env.get("JAVA_TOOL_OPTIONS").expect("JAVA_TOOL_OPTIONS");
    assert!(opts.contains("-Djavax.net.ssl.trustStore="), "{opts}");
    assert!(
        opts.contains(&format!("-Dhttps.proxyPort={PORT}")),
        "{opts}"
    );
    let log = std::fs::read_to_string(&l.keytool_log).unwrap();
    assert!(!log.is_empty(), "{log}");
    assert!(l.env.keys().all(|k| !k.starts_with("moor_")));
    let props = l.dir.path().join("gradle/gradle.properties");
    let mode = std::fs::metadata(&props).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let text = std::fs::read_to_string(&props).unwrap();
    assert!(text.contains("org.gradle.jvmargs=-Xmx2560m"), "{text}");
    assert!(
        text.contains(&format!("systemProp.https.proxyPort={PORT}")),
        "{text}"
    );
    for login in ["proxyUser", "proxyPassword"] {
        assert!(!text.contains(login), "{login}\n{text}");
    }
}

#[test]
fn without_keytool_the_java_setup_changes_nothing() {
    let l = run_launch(false);
    assert!(!l.env.contains_key("JAVA_TOOL_OPTIONS"));
    let text = std::fs::read_to_string(l.dir.path().join("gradle/gradle.properties")).unwrap();
    assert_eq!(text, "org.gradle.jvmargs=-Xmx2560m\n");
    assert!(!l.dir.path().join("home/.gradle").exists());
    is(&l.env, "HTTPS_PROXY", &format!("http://127.0.0.1:{PORT}"));
    assert_ca_vars(&l.env, &l.dir.path().join("home"));
}

/// A directory holding a `sudo` that always refuses.
fn fake_sudo(dir: &Path) -> PathBuf {
    let fake = dir.join("fakebin");
    std::fs::create_dir(&fake).unwrap();
    let sudo = fake.join("sudo");
    moor_testkit::write_executable(&sudo, "#!/bin/sh\nexit 1\n").unwrap();
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

/// Asserts every CA bundle variable points at the bundle built under `home` and Node at the proxy CA.
fn assert_ca_vars(env: &HashMap<String, String>, home: &Path) {
    let bundle = home.join(".moor-proxy/ca-bundle.pem");
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
        home.join(".moor-proxy/ca.pem").to_str().unwrap(),
    );
}

/// Asserts the variable `k` is `v`.
fn is(env: &HashMap<String, String>, k: &str, v: &str) {
    assert_eq!(env.get(k).map(String::as_str), Some(v), "{k}");
}

/// Asserts the proxy URL reaches the usual and Yarn variables, and Yarn gets no CA setting of its own and a cache under `$HOME/.cache`.
fn assert_yarn_vars(env: &HashMap<String, String>, home: &Path) {
    let proxy_url = format!("http://127.0.0.1:{PORT}");
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        is(env, name, &proxy_url);
    }
    is(env, "YARN_HTTPS_PROXY", &proxy_url);
    is(env, "YARN_HTTP_PROXY", &proxy_url);
    // Yarn 3 and Yarn 4 each reject the other's CA setting; Node's CA variable covers every Yarn.
    for name in ["YARN_CA_FILE_PATH", "YARN_HTTPS_CA_FILE_PATH"] {
        assert!(!env.contains_key(name), "{name}");
    }
    is(
        env,
        "NODE_EXTRA_CA_CERTS",
        home.join(".moor-proxy/ca.pem").to_str().unwrap(),
    );
    let home = home.display();
    is(env, "YARN_CACHE_FOLDER", &format!("{home}/.cache/yarn"));
    is(
        env,
        "YARN_GLOBAL_FOLDER",
        &format!("{home}/.cache/yarn/berry"),
    );
    is(env, "npm_config_cache", &format!("{home}/.cache/npm"));
}
