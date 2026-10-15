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
fn ssh_sh(target: &str, script: &str) -> (i32, String) {
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
    let probes: [(&str, String, bool); 10] = [
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
    assert_eq!((cleanup.0, cleanup_bin.0), (0, 0));

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
