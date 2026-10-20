//! Tests that run the Java lines under `sh` with a fake `keytool` on a hand-built PATH.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::java_lines;
use crate::config::parse_repo;
use crate::name::DirectHost;

const TOOLS: [&str; 10] = [
    "sh", "cat", "mkdir", "mv", "rm", "sed", "readlink", "dirname", "env", "tail",
];

/// A scratch directory with a `bin` holding only the tools the lines need.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("moor-java-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        for tool in TOOLS {
            let found = std::env::var("PATH")
                .unwrap()
                .split(':')
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.is_file())
                .unwrap();
            symlink(found, root.join("bin").join(tool)).unwrap();
        }
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("keytool.log")).unwrap_or_default()
    }

    /// Install a `keytool` that logs its arguments and creates the keystore it is given.
    fn fake_keytool(&self, at: &Path) {
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> {}\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -keystore|-destkeystore) : >> \"$2\";; esac\n  shift\ndone\n",
            self.root.join("keytool.log").display()
        );
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, script).unwrap();
        fs::set_permissions(at, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Run the lines, then `env`; returns the resulting environment.
    fn run(&self, extra: &[(&str, &str)]) -> HashMap<String, String> {
        self.run_direct(&[], extra)
    }

    /// [`Self::run`] with `direct` hosts.
    fn run_direct(&self, direct: &[DirectHost], extra: &[(&str, &str)]) -> HashMap<String, String> {
        let lines = java_lines(4321, "'/ca dir/proxy-ca.pem'", direct);
        let script = format!("{}\nenv", lines.join("\n"));
        let path = self.root.join("launch.sh");
        fs::write(&path, &script).unwrap();
        let syntax = Command::new("sh").arg("-n").arg(&path).output().unwrap();
        assert!(
            syntax.status.success(),
            "{}",
            String::from_utf8_lossy(&syntax.stderr)
        );
        let out = Command::new("sh")
            .arg(&path)
            .env_clear()
            .env("PATH", self.root.join("bin"))
            .env("HOME", self.home())
            .envs(extra.iter().copied())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn with_keytool(name: &str) -> Scratch {
    let s = Scratch::new(name);
    s.fake_keytool(&s.root.join("bin/keytool"));
    s
}

#[test]
fn the_properties_file_is_private_merged_and_holds_no_proxy_login() {
    let s = with_keytool("props");
    let gradle = s.home().join(".gradle");
    fs::create_dir_all(&gradle).unwrap();
    fs::write(
        gradle.join("gradle.properties"),
        "org.gradle.jvmargs=-Xmx2560m\norg.gradle.daemon=false\n",
    )
    .unwrap();
    s.run(&[]);
    let file = gradle.join("gradle.properties");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let text = fs::read_to_string(&file).unwrap();
    for want in [
        "org.gradle.jvmargs=-Xmx2560m",
        "org.gradle.daemon=false",
        "systemProp.http.proxyHost=127.0.0.1",
        "systemProp.https.proxyPort=4321",
        "systemProp.http.nonProxyHosts=127.0.0.1|localhost",
        "systemProp.javax.net.ssl.trustStorePassword=changeit",
        "systemProp.javax.net.ssl.trustStoreType=PKCS12",
    ] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    for login in ["proxyUser", "proxyPassword", "disabledSchemes"] {
        assert!(!text.contains(login), "{login}\n{text}");
    }
    let ts = gradle.join("moor-proxy-truststore.p12");
    assert!(text.contains(&format!(
        "systemProp.javax.net.ssl.trustStore={}\n",
        ts.display()
    )));
    assert!(ts.is_file());
}

#[test]
fn a_last_line_without_a_newline_stays_intact_and_the_block_starts_on_its_own_line() {
    let s = with_keytool("nonl");
    let gradle = s.home().join(".gradle");
    fs::create_dir_all(&gradle).unwrap();
    let file = gradle.join("gradle.properties");
    fs::write(&file, "org.gradle.workers.max=2").unwrap();
    s.run(&[]);
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.starts_with("org.gradle.workers.max=2\n# moor proxy: begin\n"),
        "{text}"
    );
}

#[test]
fn an_unreadable_properties_file_is_left_alone_and_nothing_is_exported() {
    let s = with_keytool("unreadable");
    let gradle = s.home().join(".gradle");
    fs::create_dir_all(&gradle).unwrap();
    let file = gradle.join("gradle.properties");
    let original = "org.gradle.jvmargs=-Xmx2560m\norg.gradle.workers.max=2";
    fs::write(&file, original).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        // Running as root: reads succeed, so the failure cannot be provoked.
        return;
    }
    let env = s.run(&[]);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    assert!(!env.contains_key("JAVA_TOOL_OPTIONS"));
}

#[test]
fn running_twice_leaves_one_block_and_no_temp_files() {
    let s = with_keytool("twice");
    s.run(&[]);
    s.run(&[]);
    let gradle = s.home().join(".gradle");
    let text = fs::read_to_string(gradle.join("gradle.properties")).unwrap();
    assert_eq!(text.matches("# moor proxy: begin").count(), 1, "{text}");
    assert_eq!(text.matches("# moor proxy: end").count(), 1, "{text}");
    assert_eq!(text.matches("http.proxyHost=").count(), 1, "{text}");
    assert_eq!(fs::read_dir(&gradle).unwrap().count(), 2);
}

#[test]
fn keytool_imports_the_proxy_ca_into_the_truststore() {
    let s = with_keytool("secret");
    s.run(&[]);
    let log = s.log();
    assert!(
        log.contains("-importcert") && log.contains("-alias moor-proxy"),
        "{log}"
    );
    assert!(log.contains("-file /ca dir/proxy-ca.pem"), "{log}");
}

#[test]
fn java_tool_options_points_at_the_truststore_and_keeps_an_existing_value() {
    let s = with_keytool("opts");
    let env = s.run(&[("JAVA_TOOL_OPTIONS", "-Dkeep=1")]);
    let ts = s.home().join(".gradle/moor-proxy-truststore.p12");
    let opts = env.get("JAVA_TOOL_OPTIONS").unwrap();
    assert!(opts.starts_with("-Dkeep=1 "), "{opts}");
    for want in [
        format!("-Djavax.net.ssl.trustStore={}", ts.display()),
        "-Djavax.net.ssl.trustStorePassword=changeit".to_owned(),
        "-Djavax.net.ssl.trustStoreType=PKCS12".to_owned(),
        "-Dhttps.proxyPort=4321".to_owned(),
        "-Dhttp.proxyPort=4321".to_owned(),
        "-Dhttp.proxyHost=127.0.0.1".to_owned(),
        "-Dhttp.nonProxyHosts=127.0.0.1|localhost".to_owned(),
    ] {
        assert!(opts.contains(&want), "{want} in {opts}");
    }
}

#[test]
fn gradle_user_home_moves_the_files() {
    let s = with_keytool("guh");
    let custom = s.root.join("custom");
    s.run(&[("GRADLE_USER_HOME", custom.to_str().unwrap())]);
    assert!(custom.join("gradle.properties").is_file());
    assert!(custom.join("moor-proxy-truststore.p12").is_file());
    assert!(!s.home().join(".gradle").exists());
}

#[test]
fn the_jdk_cacerts_beside_keytool_seed_the_truststore() {
    let s = Scratch::new("cacerts");
    let jdk = s.root.join("jdk");
    s.fake_keytool(&jdk.join("bin/keytool"));
    fs::create_dir_all(jdk.join("lib/security")).unwrap();
    fs::write(jdk.join("lib/security/cacerts"), "x").unwrap();
    symlink(jdk.join("bin/keytool"), s.root.join("bin/keytool")).unwrap();
    s.run(&[]);
    let log = s.log();
    assert!(log.contains("-importkeystore"), "{log}");
    assert!(
        log.contains(&format!(
            "-srckeystore {}",
            jdk.join("lib/security/cacerts").display()
        )),
        "{log}"
    );
}

#[test]
fn a_failing_keytool_sets_nothing_and_does_not_stop_the_prelude() {
    let s = Scratch::new("fail");
    let at = s.root.join("bin/keytool");
    fs::write(&at, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&at, fs::Permissions::from_mode(0o755)).unwrap();
    let env = s.run(&[]);
    assert!(!env.contains_key("JAVA_TOOL_OPTIONS"));
    assert!(!s.home().join(".gradle/gradle.properties").exists());
    assert!(!s.home().join(".gradle/moor-proxy-truststore.p12").exists());
}

#[test]
fn without_keytool_nothing_is_written_or_exported() {
    let s = Scratch::new("none");
    let env = s.run(&[]);
    assert!(!env.contains_key("JAVA_TOOL_OPTIONS"));
    assert!(!s.home().join(".gradle").exists());
}

/// The `proxy.direct` hosts of a repo config listing `entries`.
fn direct(entries: &str) -> Vec<DirectHost> {
    parse_repo("m.yaml", &format!("proxy: {{direct: [{entries}]}}"))
        .unwrap()
        .proxy
        .direct
}

/// The `-D` options `JAVA_TOOL_OPTIONS` holds.
fn java_opts(env: &HashMap<String, String>) -> Vec<String> {
    env.get("JAVA_TOOL_OPTIONS")
        .unwrap()
        .split(' ')
        .map(str::to_owned)
        .collect()
}

/// Asserts both schemes skip the Proxy for exactly `hosts`, in `gradle.properties` and `JAVA_TOOL_OPTIONS`.
fn assert_non_proxy(s: &Scratch, env: &HashMap<String, String>, hosts: &str) {
    let text = fs::read_to_string(s.home().join(".gradle/gradle.properties")).unwrap();
    let opts = java_opts(env);
    for scheme in ["http", "https"] {
        let line = format!("systemProp.{scheme}.nonProxyHosts={hosts}");
        assert!(text.lines().any(|l| l == line), "{line}\n{text}");
        let opt = format!("-D{scheme}.nonProxyHosts={hosts}");
        assert!(opts.contains(&opt), "{opt} in {opts:?}");
    }
}

#[test]
fn direct_hosts_join_the_gradle_and_java_non_proxy_lists() {
    let s = with_keytool("direct");
    let env = s.run_direct(&direct("dl.google.com, .gradle.org"), &[]);
    assert_non_proxy(&s, &env, "127.0.0.1|localhost|dl.google.com|*.gradle.org");
}

#[test]
fn without_direct_hosts_the_jvm_skips_the_proxy_for_loopback_only() {
    let s = with_keytool("nodirect");
    let env = s.run(&[]);
    assert_non_proxy(&s, &env, "127.0.0.1|localhost");
}
