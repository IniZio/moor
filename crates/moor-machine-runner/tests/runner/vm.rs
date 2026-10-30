use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::rig::Rig;

/// The headers for Box 7 in slot 2 of VM host `hv`, whose crabbox provider is `external`.
const VM_HOST: [(&str, &str); 6] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine", "hv"),
    ("X-Moor-Machine-Ssh", "moor@minion"),
    ("X-Moor-Machine-Slot", "2"),
    ("X-Moor-Machine-Kind", "vm-host"),
    ("X-Moor-Machine-Vm", "external"),
];

/// Stands in for crabbox on the host: logs its dir and argv to `~/crabbox.log` and prints the
/// VM's ssh words, which the fake ssh reaches as the home `vm@vm7`.
const FAKE_CRABBOX: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$HOME/crabbox.log"
case "$1" in
  warmup) sleep 1 ;;
  ssh) echo "ssh '-p' '2222' 'vm@vm7'" ;;
esac
exit 0
"#;

/// Notes where it ran and what it was given in `~/prepared`, then prints a fact.
const PREPARE: &str =
    r#"echo "$HOME $MOOR_MACHINE_KIND $MOOR_PORT_OFFSET" >> ~/prepared; echo '{"device":"vm"}'"#;

/// Notes each run in `~/released`.
const RELEASE: &str = r#"echo "$MOOR_BOX" >> ~/released"#;

/// A rig whose host has crabbox, and the VM's home.
fn vm_rig() -> (Rig, PathBuf) {
    let rig = Rig::with_args(&[
        "--prepare",
        PREPARE,
        "--release",
        RELEASE,
        "--sweep-secs",
        "1",
    ]);
    moor_testkit::write_executable(&rig.dir().join("bin/crabbox"), FAKE_CRABBOX).unwrap();
    let vm = rig.machine("vm@vm7");
    (rig, vm)
}

fn crabbox_calls(rig: &Rig) -> Vec<String> {
    lines(&rig.remote.join("crabbox.log"))
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_vm_host_claim_boots_once_and_exec_runs_in_the_vm() {
    let (rig, vm) = vm_rig();
    let (out, notes) = rig.call_progress("exec", json!({"command": "pwd"}), &VM_HOST);
    let out = out.unwrap();
    let pwd = out["output"].as_str().unwrap();
    assert!(pwd.trim_end().ends_with("vm@vm7/moor/7"), "{out}");
    assert!(
        notes.iter().any(|n| n == "booting moor-7 on hv: 0 s"),
        "{notes:?}"
    );
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    let calls = crabbox_calls(&rig);
    let warmups: Vec<_> = calls.iter().filter(|c| c.contains(" warmup ")).collect();
    assert_eq!(warmups.len(), 1, "{calls:?}");
    assert!(
        warmups[0].ends_with("moor-state/7/lease warmup --provider external --slug moor-7"),
        "{calls:?}"
    );
    assert!(calls
        .iter()
        .any(|c| c.ends_with("ssh --provider external --id moor-7")));
    assert_eq!(
        lines(&vm.join("prepared")),
        [format!("{} vm-host 0", vm.display())]
    );
    assert!(!rig.remote.join("prepared").exists());
}

#[test]
fn machine_info_on_a_vm_host_reports_the_repos_own_ports() {
    let (rig, _vm) = vm_rig();
    let info = rig.call_with("machine_info", json!({}), &VM_HOST).unwrap();
    assert_eq!(info["port_offset"], 0, "{info}");
    assert_eq!(info["kind"], "vm-host", "{info}");
    assert_eq!(info["slot"], 2, "{info}");
}

#[test]
fn words_that_turn_host_key_checking_off_delete_the_vm_and_fail() {
    let (rig, _vm) = vm_rig();
    std::fs::write(
        rig.remote.join(".ssh-G"),
        "stricthostkeychecking no\nuserknownhostsfile /dev/null\n",
    )
    .unwrap();
    let err = rig
        .call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap_err();
    assert!(
        err.contains("moor refuses the ssh options crabbox printed for this Box: they turn host key checking off"),
        "{err}"
    );
    let calls = crabbox_calls(&rig);
    assert!(
        calls
            .last()
            .unwrap()
            .ends_with("stop --provider external --id moor-7"),
        "{calls:?}"
    );
}

#[test]
fn a_boot_that_stopped_part_way_deletes_the_vm_and_boots_again() {
    let (rig, _vm) = vm_rig();
    let boxes = rig.state().join("boxes");
    std::fs::create_dir_all(&boxes).unwrap();
    let planned = json!({"ssh": "moor@minion", "slot": 2, "facts": {}, "prepared": false,
        "vm": {"provider": "external", "slug": "moor-7"}});
    std::fs::write(boxes.join("7.json"), planned.to_string()).unwrap();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    let calls = crabbox_calls(&rig);
    let verbs: Vec<&str> = calls.iter().map(|c| c.split(' ').nth(1).unwrap()).collect();
    assert_eq!(verbs, ["stop", "warmup", "ssh"], "{calls:?}");
    assert!(calls[0].ends_with("--id moor-7"), "{calls:?}");
}

#[test]
fn closing_the_box_deletes_its_vm_without_release() {
    let (rig, vm) = vm_rig();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    std::fs::create_dir_all(rig.remote.join("moor-state/7")).unwrap();
    rig.open.lock().unwrap().clear();
    let record = rig.state().join("boxes/7.json");
    eventually("the record goes", || !record.exists());
    let calls = crabbox_calls(&rig);
    assert!(
        calls
            .last()
            .unwrap()
            .ends_with("stop --provider external --id moor-7"),
        "{calls:?}"
    );
    assert!(!rig.remote.join("moor-state/7").exists());
    assert!(!rig.remote.join("released").exists());
    assert!(!vm.join("released").exists());
}
