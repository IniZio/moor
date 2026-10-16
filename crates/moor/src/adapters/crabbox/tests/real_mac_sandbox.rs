//! Opt-in check that the Seatbelt profile with a per-Task HOME keeps the owner's credentials out of
//! reach on a real Mac.
//!
//! Runs only when `MOOR_REAL_MAC_HOST` names the Mac (`user@host`). The probes print labels and
//! exit codes, never file contents.
#![expect(
    clippy::print_stderr,
    reason = "an opt-in run reports its evidence on stderr"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::driver::Driver;

use super::real_mac::{acquire, crabbox_env, fixture_repo, lease_of, Leases, HOST_VAR};
use super::Crabbox;
use crate::adapters::box_agent::nono_release::{NonoRelease, UreqFetch};
use crate::adapters::box_agent::sandbox::{self, SandboxInput};
use crate::adapters::proc::{SystemProc, ToolPaths};

/// Runs `script` under `sh` on the Mac over plain ssh and returns its exit code and stdout.
pub(super) fn ssh_sh(target: &str, script: &str) -> (i32, String) {
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", target, "sh", "-c", &quote(script)])
        .stderr(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one linear scenario: install, probe, clean up, assert"
)]
fn seatbelt_profile_with_per_task_home_denies_the_owners_credentials() {
    let Ok(target) = std::env::var(HOST_VAR) else {
        eprintln!("skip: {HOST_VAR} is not set");
        return;
    };
    let user = target.split_once('@').unwrap().0;
    let login_home = format!("/Users/{user}");
    let env = crabbox_env(&target);
    let driver = Crabbox::fake_unchecked(Arc::new(SystemProc::new(&env, &ToolPaths::default())));
    let repo = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    let mut leases = Leases {
        driver: &driver,
        target: &target,
        handles: Vec::new(),
        dirs: Vec::new(),
    };
    let acq = acquire(
        &driver,
        &mut leases,
        987_654_331,
        "realmac-sbx",
        repo.path(),
    );
    assert_eq!(acq.platform.os, "Darwin");

    let lease = lease_of(&acq);
    let workdir = acq.workdir.clone();
    let home = BoxPath::restore(&format!("{}.moor-home", workdir.as_str())).unwrap();
    let bin_dir = BoxPath::under(&lease, "nono-bin").unwrap();
    let cred = BoxPath::under(&lease, "cred").unwrap();
    let tmp = BoxPath::under(&lease, "tmp").unwrap();
    let nono = BoxPath::under(bin_dir.as_str(), "nono").unwrap();
    let profile_at = BoxPath::under(cred.as_str(), sandbox::PROFILE_FILE).unwrap();

    let dirs = [&bin_dir, &cred, &tmp, &home]
        .map(|d| quote(d.as_str()))
        .into_iter()
        .chain([quote(&format!("{}/.cache", home.as_str()))])
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(ssh_sh(&target, &format!("mkdir -p {dirs}")).0, 0);
    let nonce: String = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let marker = format!(".moor-probe-{nonce}");
    let alt_dir = format!("{login_home}/.local/state/moor-probe-alt-{nonce}");
    let other_cred_dir = format!("{login_home}/.moor-cred-0-{nonce}");
    let own_ca = format!("{}/proxy-ca.pem", cred.as_str());
    let sock = format!("{login_home}/.local/state/moor/probe-{nonce}.sock");
    // Each target is a directory plus a file in it that exists and is readable outside the sandbox,
    // so a denial inside it means something. Existing directories only get the marker file.
    let targets: [(String, String); 6] = [
        (
            format!("{login_home}/.config/moor"),
            format!("{login_home}/.config/moor/{marker}"),
        ),
        (
            format!("{login_home}/.local/state/moor"),
            format!("{login_home}/.local/state/moor/{marker}"),
        ),
        (alt_dir.clone(), format!("{alt_dir}/{marker}")),
        (
            format!("{login_home}/.config/agent37"),
            format!("{login_home}/.config/agent37/{marker}"),
        ),
        (
            format!("{login_home}/.config/crabbox"),
            format!("{login_home}/.config/crabbox/{marker}"),
        ),
        (
            other_cred_dir.clone(),
            format!("{other_cred_dir}/proxy-ca.pem"),
        ),
    ];
    let setup = targets
        .iter()
        .enumerate()
        .fold(String::new(), |mut acc, (i, (dir, file))| {
            let _ = writeln!(
                acc,
                "p={d}; top=; while [ ! -e \"$p\" ]; do top=$p; p=$(dirname \"$p\"); done; echo \"top{i} $top\"; \
                 mkdir -p {d} && touch {f}; test -r {f}; echo \"outside{i} $?\"",
                d = quote(dir),
                f = quote(file),
            );
            acc
        });
    let (_, setup_out) = ssh_sh(
        &target,
        &format!(
            "{setup}touch {}; test -r {}; echo \"outside_own $?\"",
            quote(&own_ca),
            quote(&own_ca)
        ),
    );
    // Outermost directory the test created for each target; empty when it already existed.
    let tops: Vec<String> = (0..targets.len())
        .map(|i| {
            setup_out
                .lines()
                .find_map(|l| l.strip_prefix(&format!("top{i} ")))
                .unwrap_or("")
                .to_owned()
        })
        .collect();
    let outside_ok = |key: &str| setup_out.lines().any(|l| l.trim() == format!("{key} 0"));
    let listener = ssh_sh(
        &target,
        &format!(
            "nohup /usr/bin/nc -lkU {s} >/dev/null 2>&1 </dev/null & echo $!; sleep 1",
            s = quote(&sock)
        ),
    );
    let listener_pid: String = listener
        .1
        .trim()
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let socket_outside = ssh_sh(
        &target,
        &format!("/usr/bin/nc -U {} </dev/null >/dev/null 2>&1", quote(&sock)),
    )
    .0;
    let moor_paths = vec![alt_dir];

    let bytes = NonoRelease::new(Arc::new(UreqFetch), None)
        .binary(&acq.platform.os, &acq.platform.arch)
        .unwrap();
    driver
        .put_file(&acq.handle, &nono, &bytes, 0o755, repo.path())
        .unwrap();
    let text = sandbox::profile(&SandboxInput {
        os: "Darwin",
        workdir: &workdir,
        home: &home,
        login_home: &BoxPath::restore(&login_home).unwrap(),
        cred: &cred,
        tmp: &tmp,
        moor_paths: &moor_paths,
        devices: &[],
        env_names: Vec::new(),
        set_vars: vec![("IS_SANDBOX".to_owned(), "1".to_owned())],
    });
    driver
        .put_file(
            &acq.handle,
            &profile_at,
            text.as_bytes(),
            0o600,
            repo.path(),
        )
        .unwrap();

    let owner = |rel: &str| quote(&format!("{login_home}/{rel}"));
    let mut probes: Vec<(&str, String, bool)> = vec![
        (
            "gh_hosts_test_r",
            format!("test -r {}", owner(".config/gh/hosts.yml")),
            false,
        ),
        (
            "gh_hosts_cat",
            format!("cat {} >/dev/null 2>&1", owner(".config/gh/hosts.yml")),
            false,
        ),
        (
            "ssh_ls",
            format!("ls {} >/dev/null 2>&1", owner(".ssh")),
            false,
        ),
        (
            "docker_config_test_r",
            format!("test -r {}", owner(".docker/config.json")),
            false,
        ),
        (
            "docker_dir_ls",
            format!("ls {} >/dev/null 2>&1", owner(".docker")),
            false,
        ),
        (
            "docker_info",
            "/usr/local/bin/docker info >/dev/null 2>&1".to_owned(),
            false,
        ),
        (
            "home_write",
            "touch \"$HOME/.cache/probe\" && test -w \"$HOME/.cache/probe\"".to_owned(),
            true,
        ),
        (
            "workdir_write",
            format!("touch {}", quote(&format!("{}/probe", workdir.as_str()))),
            true,
        ),
        (
            "home_is_task_home",
            format!("[ \"$HOME\" = {} ]", quote(home.as_str())),
            true,
        ),
        ("is_sandbox", "[ \"$IS_SANDBOX\" = 1 ]".to_owned(), true),
    ];
    let [(cfg_dir, cfg_file), (state_dir, state_file), (_, alt_file), (_, a37_file), (_, cb_file), (_, other_file)] =
        &targets;
    probes.extend([
        (
            "moor_config_test_r",
            format!("test -r {}", quote(cfg_file)),
            false,
        ),
        (
            "moor_config_ls",
            format!("ls {} >/dev/null 2>&1", quote(cfg_dir)),
            false,
        ),
        (
            "moor_state_test_r",
            format!("test -r {}", quote(state_file)),
            false,
        ),
        (
            "moor_state_ls",
            format!("ls {} >/dev/null 2>&1", quote(state_dir)),
            false,
        ),
        (
            "moor_state_override_test_r",
            format!("test -r {}", quote(alt_file)),
            false,
        ),
        (
            "agent37_config_test_r",
            format!("test -r {}", quote(a37_file)),
            false,
        ),
        (
            "crabbox_config_test_r",
            format!("test -r {}", quote(cb_file)),
            false,
        ),
        (
            "other_task_cred_test_r",
            format!("test -r {}", quote(other_file)),
            false,
        ),
        (
            "own_cred_test_r",
            format!("test -r {}", quote(&own_ca)),
            true,
        ),
        (
            "controller_socket_connect",
            format!("/usr/bin/nc -U {} </dev/null >/dev/null 2>&1", quote(&sock)),
            false,
        ),
    ]);
    let docker_outside = ssh_sh(
        &target,
        "/usr/local/bin/docker info >/dev/null 2>&1; echo $?",
    )
    .1
    .trim()
    .to_owned();
    eprintln!("probe docker_info_outside_sandbox: exit {docker_outside}");
    let docker_reachable = docker_outside == "0";
    let inner = probes
        .iter()
        .fold(String::new(), |mut acc, (label, cmd, _)| {
            let _ = writeln!(acc, "{cmd}; echo \"{label} $?\"");
            acc
        });
    let outer = format!(
        "cd {} && PATH=/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin HOME={} {} -s run --profile {} -- sh -c {}",
        quote(workdir.as_str()),
        quote(home.as_str()),
        quote(nono.as_str()),
        quote(profile_at.as_str()),
        quote(&inner),
    );
    let (outer_code, stdout) = ssh_sh(&target, &format!("{outer} 2>&1"));
    eprintln!("sandbox run: exit {outer_code}");
    if codes_missing(&stdout) {
        eprintln!("sandbox output:\n{stdout}");
    }
    let codes: BTreeMap<&str, i32> = stdout
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter_map(|(k, v)| Some((k, v.trim().parse().ok()?)))
        .collect();

    let cleanup = ssh_sh(
        &target,
        &format!(
            "rm -rf -- {}",
            quote(&format!("{}.moor-home", workdir.as_str()))
        ),
    );
    let cleanup_bin = ssh_sh(&target, &format!("rm -rf -- {}", quote(bin_dir.as_str())));
    let removals = targets
        .iter()
        .zip(&tops)
        .map(|((_, file), top)| {
            if top.is_empty() {
                format!("rm -f -- {}", quote(file))
            } else {
                format!("rm -rf -- {}", quote(top))
            }
        })
        .chain([format!("rm -f -- {} {}", quote(&own_ca), quote(&sock))])
        .collect::<Vec<_>>()
        .join("; ");
    let kill = if listener_pid.is_empty() {
        String::new()
    } else {
        format!("kill {listener_pid} 2>/dev/null; ")
    };
    let cleanup_probes = ssh_sh(&target, &format!("{kill}{removals}"));
    assert_eq!((cleanup.0, cleanup_bin.0, cleanup_probes.0), (0, 0, 0));

    for i in 0..targets.len() {
        eprintln!(
            "setup target {i}: outside test -r ok = {}",
            outside_ok(&format!("outside{i}"))
        );
    }
    eprintln!(
        "setup own_ca: outside test -r ok = {}",
        outside_ok("outside_own")
    );
    eprintln!("setup socket: outside nc -U exit {socket_outside}");
    for i in 0..targets.len() {
        assert!(
            outside_ok(&format!("outside{i}")),
            "target {i} not readable outside the sandbox"
        );
    }
    assert!(
        outside_ok("outside_own"),
        "own CA not readable outside the sandbox"
    );
    assert_eq!(
        socket_outside, 0,
        "listener not connectable outside the sandbox"
    );

    for (label, _, allowed) in &probes {
        let code = codes.get(label).copied();
        eprintln!("probe {label}: exit {code:?}");
        if *label == "docker_info" && !docker_reachable {
            eprintln!(
                "probe docker_info: INCONCLUSIVE, docker is not reachable outside the sandbox"
            );
            continue;
        }
        let ok = match (code, allowed) {
            (Some(0), true) => true,
            (Some(c), false) => c != 0,
            _ => false,
        };
        assert!(
            ok,
            "probe {label}: exit {code:?}, expected {}",
            if *allowed { "0" } else { "nonzero" }
        );
    }
}

/// True when the sandboxed run printed no probe line, so its raw output is worth showing.
fn codes_missing(stdout: &str) -> bool {
    !stdout.lines().any(|l| l.starts_with("home_write "))
}
