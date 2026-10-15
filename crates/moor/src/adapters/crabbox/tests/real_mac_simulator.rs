//! Opt-in check that a per-Task iOS simulator works under the Seatbelt profile on a real Mac and
//! leaves the owner's simulators alone.
//!
//! Runs only when `MOOR_REAL_MAC_HOST` names the Mac (`user@host`). The test creates its own two
//! devices (`moor-<id>`), never boots, shuts down, deletes or captures any other device, and
//! deletes its devices on every exit path.
#![expect(
    clippy::print_stderr,
    reason = "an opt-in run reports its evidence on stderr"
)]

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

const DEVICE_TYPE: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro";

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

/// Deletes the test's simulators when dropped, so a failed assertion leaves none behind.
struct Devices<'a> {
    target: &'a str,
    udids: Vec<String>,
}

impl Devices<'_> {
    fn remove_all(&self) {
        for udid in &self.udids {
            let u = quote(udid);
            let _ = ssh_sh(
                self.target,
                &format!("xcrun simctl shutdown {u}; xcrun simctl delete {u}"),
            );
        }
    }
}

impl Drop for Devices<'_> {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// Creates a device named `name` on the newest installed iOS runtime and returns its UDID.
fn create_device(target: &str, name: &str) -> String {
    let script = format!(
        "rt=$(xcrun simctl list runtimes | grep -o 'com.apple.CoreSimulator.SimRuntime.iOS-[0-9-]*' \
         | sort -t- -k2,2n -k3,3n | tail -1); xcrun simctl create {} {DEVICE_TYPE} \"$rt\"",
        quote(name)
    );
    let (code, out) = ssh_sh(target, &script);
    assert_eq!(code, 0, "simctl create {name} failed: {out}");
    out.trim().to_owned()
}

/// Booted devices that are not the test's own, so the owner's set can be compared across the run.
fn owner_booted(target: &str) -> String {
    ssh_sh(
        target,
        "xcrun simctl list devices booted | grep -v moor-; true",
    )
    .1
}

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one linear scenario: create devices, capture proof in the sandbox, probe, clean up"
)]
fn per_task_simulator_captures_proof_inside_the_sandbox_and_leaves_the_owners_devices_alone() {
    let Ok(target) = std::env::var(HOST_VAR) else {
        eprintln!("skip: {HOST_VAR} is not set");
        return;
    };
    let user = target.split_once('@').unwrap().0;
    let login_home = format!("/Users/{user}");
    let devices_dir = format!("{login_home}/Library/Developer/CoreSimulator/Devices");

    let before = owner_booted(&target);
    eprintln!("owner booted before:\n{before}");

    let id = std::process::id();
    let mut devices = Devices {
        target: &target,
        udids: Vec::new(),
    };
    let udid_a = create_device(&target, &format!("moor-{id}"));
    devices.udids.push(udid_a.clone());
    let udid_b = create_device(&target, &format!("moor-{}", id + 1));
    devices.udids.push(udid_b.clone());
    eprintln!("created devices: A={udid_a} B={udid_b}");
    let (boot_code, boot_out) = ssh_sh(
        &target,
        &format!(
            "xcrun simctl boot {a} 2>&1 && xcrun simctl bootstatus {a} -b 2>&1 | tail -n 1",
            a = quote(&udid_a)
        ),
    );
    assert_eq!(boot_code, 0, "boot of the test device failed: {boot_out}");

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
        987_654_332,
        "realmac-sim",
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
        moor_paths: &[],
        env_names: Vec::new(),
        set_vars: vec![("IS_SANDBOX".to_owned(), "1".to_owned())],
        sim_udid: Some(&udid_a),
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

    // Runs `inner` under `sh -c` inside nono with the real profile; returns exit code and the
    // merged stdout and stderr.
    let sandboxed = |inner: &str| {
        let outer = format!(
            "cd {} && PATH=/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin HOME={} {} -s run --profile {} -- sh -c {} 2>&1",
            quote(workdir.as_str()),
            quote(home.as_str()),
            quote(nono.as_str()),
            quote(profile_at.as_str()),
            quote(inner),
        );
        ssh_sh(&target, &outer)
    };
    // simctl resolves a relative output path in the CoreSimulator service, not the caller, so the
    // paths passed to it are absolute.

    let a = quote(&udid_a);
    let b = quote(&udid_b);
    let (proof_code, proof_out) = sandboxed(&format!(
        "mkdir -p .moor/proof && xcrun simctl io {a} screenshot \"$PWD/.moor/proof/shot.png\"; echo \"shot $?\"; \
         xcrun simctl io {a} recordVideo --codec=h264 \"$PWD/.moor/proof/clip.mp4\" & \
         sleep 3; kill -INT $!; wait; echo \"clip done\""
    ));
    eprintln!("sandbox proof capture: exit {proof_code}\n{proof_out}");

    let wd = quote(workdir.as_str());
    let (_, facts) = ssh_sh(
        &target,
        &format!(
            "cd {wd} && echo \"png_magic $(head -c 4 .moor/proof/shot.png | xxd -p)\"; \
             echo \"png_size $(wc -c < .moor/proof/shot.png)\"; \
             echo \"mp4_tag $(dd if=.moor/proof/clip.mp4 bs=1 skip=4 count=4 2>/dev/null)\"; \
             echo \"mp4_size $(wc -c < .moor/proof/clip.mp4)\""
        ),
    );
    eprintln!("proof files:\n{facts}");
    let fact = |key: &str| {
        facts
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key} ")))
            .unwrap_or("")
            .trim()
            .to_owned()
    };

    let (ls_code, ls_out) = sandboxed(&format!(
        "ls {d}/{b} >/dev/null 2>&1; echo \"ls_other $?\"; cat {d}/{b}/device.plist >/dev/null 2>&1; echo \"cat_other $?\"",
        d = quote(&devices_dir)
    ));
    eprintln!("sandbox other-device data: exit {ls_code}\n{ls_out}");

    // The sandbox profile allows every mach lookup, so CoreSimulatorService accepts commands from
    // inside it. Record the outcome; the test does not assert it.
    let (_, boot_b) = sandboxed(&format!("xcrun simctl boot {b}; echo \"rc=$?\""));
    let rc = boot_b
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("rc="))
        .unwrap_or("?");
    eprintln!("control-plane: sandbox booted another device: rc={rc}");
    eprintln!("control-plane: simctl output: {}", boot_b.trim());

    let cleanup_home = ssh_sh(&target, &format!("rm -rf -- {}", quote(home.as_str())));
    let cleanup_bin = ssh_sh(&target, &format!("rm -rf -- {}", quote(bin_dir.as_str())));
    devices.remove_all();
    let (_, remaining) = ssh_sh(&target, "xcrun simctl list devices | grep moor-; true");
    let after = owner_booted(&target);
    eprintln!("owner booted after:\n{after}");

    assert_eq!((cleanup_home.0, cleanup_bin.0), (0, 0));
    assert!(
        proof_out.contains("shot 0"),
        "screenshot failed in the sandbox"
    );
    assert_eq!(fact("png_magic"), "89504e47", "screenshot is not a PNG");
    assert!(fact("png_size").parse::<u64>().unwrap_or(0) > 0);
    assert_eq!(fact("mp4_tag"), "ftyp", "recording is not an MP4");
    assert!(fact("mp4_size").parse::<u64>().unwrap_or(0) > 0);
    assert!(
        !ls_out.contains("ls_other 0"),
        "sandbox listed another device's data directory"
    );
    assert!(
        !ls_out.contains("cat_other 0"),
        "sandbox read another device's data"
    );
    assert!(
        !remaining.contains(&udid_a) && !remaining.contains(&udid_b),
        "test devices still listed: {remaining}"
    );
    assert_eq!(after, before, "the owner's booted devices changed");
}
