//! Opt-in check on a real Mac that the macOS sandbox reaches HTTPS only through the Proxy and
//! trusts it only through the per-Task CA file.
//!
//! Runs only when `MOOR_REAL_MAC_HOST` names the Mac (`user@host`). The Proxy runs here, a
//! reverse ssh tunnel carries it to the Mac's loopback, and the probes print labels and exit
//! codes, never the nonce or the environment. The injected credential is a placeholder.
#![expect(
    clippy::print_stderr,
    reason = "an opt-in run reports its evidence on stderr"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use moor_core::boxscript::quote;
use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::seam::clock::Ts;
use moor_core::{BoxPath, Secret, TaskId};

use super::real_mac::{HOST_VAR, WORK_ROOT};
use super::real_mac_sandbox::ssh_sh;
use crate::adapters::box_agent::nono_release::{NonoRelease, UreqFetch};
use crate::adapters::box_agent::sandbox::{self, SandboxInput};
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::testing::clock::FakeClock;

/// Placeholder the Proxy injects; it is not a credential.
const DUMMY: &str = "placeholder-not-a-token";
/// Header the placeholder travels in; the targets ignore it.
const DUMMY_HEADER: &str = "X-Moor-Test";
/// A public repository for `git ls-remote`.
const REPO: &str = "https://github.com/octocat/Hello-World.git";
/// Directories the sandbox may run tools from.
const SYS_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin";

/// Removes what the check made on the Mac and ends the tunnel, also when an assertion fails.
struct Guard<'a> {
    /// Mac as `user@host`.
    target: &'a str,
    /// Paths to remove on the Mac.
    paths: Vec<String>,
    /// The reverse tunnel.
    tunnel: Option<Child>,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(t) = self.tunnel.as_mut() {
            let _ = t.kill();
            let _ = t.wait();
        }
        let rm = self.paths.iter().map(|p| quote(p)).collect::<Vec<_>>();
        if !rm.is_empty() {
            ssh_sh(self.target, &format!("rm -rf -- {}", rm.join(" ")));
        }
    }
}

/// Writes `bytes` to `path` on the Mac with `mode`, over ssh stdin.
fn put(target: &str, path: &str, bytes: &[u8], mode: &str) {
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", target])
        .arg(format!(
            "umask 077; cat > {p} && chmod {mode} {p}",
            p = quote(path)
        ))
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    assert!(child.wait().unwrap().success(), "could not write {path}");
}

/// One loopback port that is free right now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Rule that intercepts `hosts` and injects the placeholder header.
fn rule(hosts: &[&str]) -> ResolvedRule {
    ResolvedRule {
        name: "probe".to_owned(),
        hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
        credential: "TOKEN".to_owned(),
        inject: ResolvedInject::Header {
            header: DUMMY_HEADER.to_owned(),
            form: "{secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "*".to_owned(),
            path: "/**".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// Probe lines run inside the sandbox: `<label> <exit code> [extra]`.
fn probes(repo: &str) -> String {
    let proxy = r#"HTTPS_PROXY="http://moor:$(cat "$C/proxy-nonce")@127.0.0.1:$PORT""#;
    let strip = "env -u CURL_CA_BUNDLE -u SSL_CERT_FILE -u GIT_SSL_CAINFO -u NODE_EXTRA_CA_CERTS";
    let curl = "/usr/bin/curl -sS -m 40 -o /dev/null -w '%{http_code}'";
    let js = "fetch('https://example.com').then(r=>process.exit(r.status===200?0:3)).catch(()=>process.exit(2))";
    let mut s = String::new();
    let mut line = |label: &str, cmd: String| {
        let _ = writeln!(s, "out=$({cmd} 2>/dev/null); echo \"{label} $? $out\"");
    };
    line(
        "curl_ca",
        format!("{proxy} CURL_CA_BUNDLE=\"$C/ca-bundle.pem\" SSL_CERT_FILE=\"$C/ca-bundle.pem\" {curl} https://example.com"),
    );
    line(
        "curl_no_ca",
        format!("{proxy} {strip} {curl} https://example.com"),
    );
    line(
        "curl_system_ca_only",
        format!("{proxy} CURL_CA_BUNDLE=/etc/ssl/cert.pem SSL_CERT_FILE=/etc/ssl/cert.pem {curl} https://example.com"),
    );
    line(
        "git_ca",
        format!("{proxy} GIT_TERMINAL_PROMPT=0 GIT_SSL_CAINFO=\"$C/ca-bundle.pem\" /usr/bin/git ls-remote {repo} HEAD"),
    );
    line(
        "git_no_ca",
        format!("{proxy} GIT_TERMINAL_PROMPT=0 {strip} /usr/bin/git ls-remote {repo} HEAD"),
    );
    line(
        "node_ca",
        format!("{proxy} NODE_USE_ENV_PROXY=1 NODE_EXTRA_CA_CERTS=\"$C/proxy-ca.pem\" \"$NODE\" -e \"{js}\""),
    );
    line(
        "node_no_ca",
        format!("{proxy} NODE_USE_ENV_PROXY=1 {strip} \"$NODE\" -e \"{js}\""),
    );
    line(
        "direct_curl",
        format!("{strip} {curl} --noproxy '*' https://example.com"),
    );
    line("direct_ip", "/usr/bin/nc -z -G 8 1.1.1.1 443".to_owned());
    s
}

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one linear scenario: start, tunnel, probe, compare, clean up"
)]
fn sandboxed_mac_reaches_https_only_through_the_proxy_and_its_ca_file() {
    let Ok(target) = std::env::var(HOST_VAR) else {
        eprintln!("skip: {HOST_VAR} is not set");
        return;
    };
    let user = target.split_once('@').unwrap().0;
    let login_home = format!("/Users/{user}");
    let tag = format!("{:x}", std::process::id());
    let base = format!("{WORK_ROOT}/moor-proxy-{tag}");
    let cred = format!("{login_home}/.moor-cred-0-{tag}");
    let mut guard = Guard {
        target: &target,
        paths: vec![base.clone(), cred.clone()],
        tunnel: None,
    };

    // The Proxy: intercepts the two targets with a placeholder credential.
    let state = tempfile::tempdir().unwrap();
    let task = TaskId::new(41);
    let task_ca = ca::load_or_create(state.path(), task).unwrap();
    let nonce_text = format!("n{tag}ABCDEFGHIJKLMNOPQRSTUV");
    let nonce = Secret::new(nonce_text.clone().into_bytes());
    let rules = [rule(&["example.com", "github.com"])];
    let source = RuleSource {
        config_key: "credentials.TOKEN".to_owned(),
        fetch: Arc::new(|| Ok(Secret::new(DUMMY.as_bytes().to_vec()))),
        basic_user: None,
    };
    let capture = Arc::new(MoorCapture::new(
        BTreeMap::from([("probe".to_owned(), source)]),
        Arc::new(FakeClock::new(Ts::from_millis(1_000_000))),
    ));
    let proxy = NonoProxy::start(
        build_config(&NonoSpec {
            rules: &rules,
            nonce: &nonce,
            ca: &task_ca,
            ca_dir: &ca::ca_dir(state.path(), task),
            upstream_ca: None,
            chain: None,
        }),
        capture,
    )
    .unwrap();
    eprintln!("proxy: started on loopback");

    // The tunnel puts the Proxy on the Mac's loopback.
    let box_port = free_port();
    guard.tunnel = Some(
        Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-N",
                "-R",
                &format!("{box_port}:127.0.0.1:{}", proxy.port()),
                &target,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    // Mac side files.
    let work = format!("{base}/work");
    let home = format!("{base}/home");
    let tmp = format!("{base}/tmp");
    let nono = format!("{base}/bin/nono");
    let profile_at = format!("{cred}/{}", sandbox::PROFILE_FILE);
    let mk = [
        &work,
        &home,
        &tmp,
        &format!("{base}/bin"),
        &format!("{home}/.cache"),
        &cred,
    ]
    .map(|d| quote(d))
    .join(" ");
    assert_eq!(
        ssh_sh(
            &target,
            &format!("mkdir -p {mk} && chmod 700 {}", quote(&cred))
        )
        .0,
        0
    );
    let bytes = NonoRelease::new(Arc::new(UreqFetch), None)
        .binary("Darwin", "arm64")
        .unwrap();
    put(&target, &nono, &bytes, "755");
    put(
        &target,
        &format!("{cred}/proxy-ca.pem"),
        task_ca.cert_pem().as_bytes(),
        "644",
    );
    put(
        &target,
        &format!("{cred}/proxy-nonce"),
        nonce_text.as_bytes(),
        "600",
    );
    let text = sandbox::profile(&SandboxInput {
        os: "Darwin",
        workdir: &BoxPath::restore(&work).unwrap(),
        home: &BoxPath::restore(&home).unwrap(),
        login_home: &BoxPath::restore(&login_home).unwrap(),
        cred: &BoxPath::restore(&cred).unwrap(),
        tmp: &BoxPath::restore(&tmp).unwrap(),
        moor_paths: &[],
        env_names: Vec::new(),
        set_vars: Vec::new(),
        devices: &[],
    });
    put(&target, &profile_at, text.as_bytes(), "600");
    let (c, _) = ssh_sh(
        &target,
        &format!(
            "cat /etc/ssl/cert.pem {c}/proxy-ca.pem > {c}/ca-bundle.pem",
            c = quote(&cred)
        ),
    );
    assert_eq!(c, 0);

    // The tunnel needs a moment before the Mac can use it.
    let tunnel_up = (0..20).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        ssh_sh(&target, &format!("/usr/bin/nc -z 127.0.0.1 {box_port}")).0 == 0
    });
    eprintln!("tunnel: Mac loopback port reachable = {tunnel_up}");
    assert!(tunnel_up);

    // Trust state before.
    let trust = format!(
        "fp=$(/usr/bin/openssl x509 -in {ca}/proxy-ca.pem -noout -fingerprint -sha256 | sed 's/.*=//; s/://g'); \
         k=$( (security find-certificate -a -Z; security find-certificate -a -Z /Library/Keychains/System.keychain) 2>/dev/null); \
         echo \"keychain_certs $(echo \"$k\" | grep -c 'SHA-256 hash')\"; \
         echo \"keychain_ca_matches $(echo \"$k\" | grep -ci \"$fp\")\"; \
         echo \"system_bundle_sha $(/usr/bin/shasum -a 256 /etc/ssl/cert.pem | cut -d' ' -f1)\"",
        ca = quote(&cred)
    );
    let before = ssh_sh(&target, &trust).1;

    // Node, if the Mac has one.
    let node = ssh_sh(
        &target,
        "for n in /opt/homebrew/bin/node /usr/local/bin/node $HOME/.nvm/versions/node/*/bin/node; do [ -x \"$n\" ] && echo \"$n\"; done | tail -1",
    )
    .1
    .trim()
    .to_owned();
    eprintln!(
        "node: {}",
        if node.is_empty() {
            "absent, node probes skipped"
        } else {
            "found"
        }
    );

    // Outside the sandbox, direct egress works, so a block inside means something.
    let outside = (0..3)
        .map(|_| {
            ssh_sh(
                &target,
                "/usr/bin/curl -sS -m 20 -o /dev/null https://example.com",
            )
            .0
        })
        .find(|c| *c == 0)
        .unwrap_or(-1);
    eprintln!("direct egress outside sandbox: curl exit {outside}");

    let inner = format!(
        "C={}; PORT={box_port}; NODE={}; {}",
        quote(&cred),
        quote(&node),
        probes(REPO)
    );
    let outer = format!(
        "cd {} && PATH={SYS_PATH}{} HOME={} {} -s run --profile {} -- sh -c {} 2>&1",
        quote(&work),
        if node.is_empty() {
            String::new()
        } else {
            format!(":{}", node.rsplit_once('/').unwrap().0)
        },
        quote(&home),
        quote(&nono),
        quote(&profile_at),
        quote(&inner),
    );
    let (code, out) = ssh_sh(&target, &outer);
    eprintln!("sandbox run: exit {code}");
    let rows: BTreeMap<&str, (i32, &str)> = out
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, ' ');
            let label = it.next()?;
            let rc = it.next()?.parse().ok()?;
            Some((label, (rc, it.next().unwrap_or("").trim())))
        })
        .collect();
    let after = ssh_sh(&target, &trust).1;
    drop(guard);
    proxy.stop();

    for (label, (rc, extra)) in &rows {
        let shown = if label.starts_with("curl") {
            *extra
        } else {
            ""
        };
        eprintln!("probe {label}: exit {rc} {shown}");
    }
    eprintln!("trust before:\n{before}trust after:\n{after}");
    let rc = |l: &str| rows.get(l).map(|r| r.0);
    assert_eq!(outside, 0, "direct egress must work outside the sandbox");
    assert_eq!(rows.get("curl_ca"), Some(&(0, "200")));
    assert_eq!(rc("curl_no_ca"), Some(60));
    assert_eq!(rc("curl_system_ca_only"), Some(60));
    assert_eq!(rc("git_ca"), Some(0));
    assert!(rc("git_no_ca").is_some_and(|c| c != 0));
    if !node.is_empty() {
        assert_eq!(rc("node_ca"), Some(0));
        assert!(rc("node_no_ca").is_some_and(|c| c != 0));
    }
    assert!(rc("direct_curl").is_some_and(|c| c != 0));
    assert!(rc("direct_ip").is_some_and(|c| c != 0));
    assert_eq!(before, after, "keychain or system bundle changed");
    assert!(before.contains("keychain_ca_matches 0"));
}
