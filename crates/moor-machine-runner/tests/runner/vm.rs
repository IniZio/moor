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
/// VM's ssh words, which the fake ssh reaches as the home `vm@vm7`. `~/leases.json` is what
/// `list` prints, `~/no-pause` makes `pause` refuse, `~/stop-fails` is the error `stop` fails
/// with, and `~/stop-gone` makes `stop` say the lease is gone. Like crabbox, it refuses a flag it
/// does not know.
const FAKE_CRABBOX: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$HOME/crabbox.log"
for a in "$@"; do
  case "$a" in
    --provider|--id|--slug|--json) ;;
    --*) echo "flag provided but not defined: $a" >&2; exit 2 ;;
  esac
done
case "$1" in
  warmup) sleep 1 ;;
  ssh) echo "ssh '-p' '2222' 'vm@vm7'" ;;
  list) cat "$HOME/leases.json" 2>/dev/null || echo '[]' ;;
  pause) if [ -e "$HOME/no-pause" ]; then
      echo "provider=$3 does not support pause" >&2; exit 2
    fi ;;
  stop) if [ -e "$HOME/stop-fails" ]; then cat "$HOME/stop-fails" >&2; exit 1; fi
    if [ -e "$HOME/stop-gone" ]; then echo "lease not found" >&2; exit 4; fi ;;
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

/// [`VM_HOST`] with the controller serving Box port 8081 on the host at 8101, slot 2's.
const VM_HOST_PORTS: [(&str, &str); 7] = [
    VM_HOST[0],
    VM_HOST[1],
    VM_HOST[2],
    VM_HOST[3],
    VM_HOST[4],
    VM_HOST[5],
    ("X-Moor-Machine-Ports", "8101"),
];

fn verbs(rig: &Rig) -> Vec<String> {
    crabbox_calls(rig)
        .iter()
        .map(|c| c.split(' ').nth(1).unwrap().to_owned())
        .collect()
}

fn suspend(rig: &Rig) {
    rig.asleep.lock().unwrap().push(7);
    let record = rig.state().join("boxes/7.json");
    eventually("the Box is suspended", || {
        std::fs::read_to_string(&record).is_ok_and(|r| r.contains("suspended_at\":1"))
    });
}

fn suspended_record(rig: &Rig) -> serde_json::Value {
    let raw = std::fs::read_to_string(rig.state().join("boxes/7.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn suspending_runs_the_hook_then_pauses_the_vm_and_waking_resumes_it() {
    let rig = Rig::with_args(&[
        "--prepare",
        PREPARE,
        "--suspend",
        "echo $MOOR_BOX >> ~/suspended",
        "--sweep-secs",
        "1",
    ]);
    moor_testkit::write_executable(&rig.dir().join("bin/crabbox"), FAKE_CRABBOX).unwrap();
    let vm = rig.machine("vm@vm7");
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    suspend(&rig);
    assert_eq!(lines(&vm.join("suspended")), ["7"]);
    assert_eq!(
        verbs(&rig).last().unwrap(),
        "pause",
        "{:?}",
        crabbox_calls(&rig)
    );
    assert_eq!(suspended_record(&rig)["vm"]["paused"], true);
    rig.asleep.lock().unwrap().clear();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    assert_eq!(
        verbs(&rig),
        ["warmup", "ssh", "pause", "resume", "ssh"],
        "{:?}",
        crabbox_calls(&rig)
    );
    assert_eq!(lines(&vm.join("prepared")).len(), 2);
    assert!(suspended_record(&rig)["vm"].get("paused").is_none());
}

#[test]
fn a_resumed_vm_whose_words_fail_the_pin_check_is_deleted() {
    let rig = Rig::with_args(&["--prepare", PREPARE, "--sweep-secs", "1"]);
    moor_testkit::write_executable(&rig.dir().join("bin/crabbox"), FAKE_CRABBOX).unwrap();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    suspend(&rig);
    rig.asleep.lock().unwrap().clear();
    std::fs::write(
        rig.remote.join(".ssh-G"),
        "stricthostkeychecking no\nuserknownhostsfile /dev/null\n",
    )
    .unwrap();
    let err = rig
        .call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap_err();
    assert!(err.contains("turn host key checking off"), "{err}");
    assert_eq!(verbs(&rig)[3..], ["resume", "ssh", "stop"]);
}

#[test]
fn a_provider_that_cannot_pause_leaves_the_vm_running_and_the_suspend_succeeds() {
    let rig = Rig::with_args(&["--prepare", PREPARE, "--sweep-secs", "1"]);
    moor_testkit::write_executable(&rig.dir().join("bin/crabbox"), FAKE_CRABBOX).unwrap();
    std::fs::write(rig.remote.join("no-pause"), "").unwrap();
    rig.machine("vm@vm7");
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    suspend(&rig);
    assert!(suspended_record(&rig)["vm"].get("paused").is_none());
    rig.asleep.lock().unwrap().clear();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    assert_eq!(verbs(&rig), ["warmup", "ssh", "pause"]);
}

#[test]
fn reconcile_deletes_only_orphan_vms_of_closed_boxes() {
    let rig = Rig::with_args(&["--sweep-secs", "1", "--reconcile-secs", "1"]);
    moor_testkit::write_executable(&rig.dir().join("bin/crabbox"), FAKE_CRABBOX).unwrap();
    let boxes = rig.state().join("boxes");
    std::fs::create_dir_all(&boxes).unwrap();
    let kept = json!({"ssh": "moor@minion", "slot": 1, "facts": {}, "prepared": false,
        "vm": {"provider": "external", "slug": "moor-9"}});
    std::fs::write(boxes.join("9.json"), kept.to_string()).unwrap();
    rig.open.lock().unwrap().extend([9]);
    let leases = json!([
        {"id": 1, "name": "moor-12", "labels": {"slug": "moor-12"}},
        {"slug": "moor-7"},
        {"name": "moor-9"},
        {"name": "moor-012"},
        {"name": "other-13"},
        {"labels": {"slug": "moor-x"}}
    ]);
    std::fs::write(rig.remote.join("leases.json"), leases.to_string()).unwrap();
    *rig.targets.lock().unwrap() = Some(vec!["moor@minion#external".to_owned()]);
    eventually("the orphan VM is deleted", || {
        crabbox_calls(&rig).iter().any(|c| c.contains(" stop "))
    });
    std::thread::sleep(Duration::from_secs(2));
    let stops: Vec<String> = crabbox_calls(&rig)
        .into_iter()
        .filter(|c| c.contains(" stop "))
        .collect();
    assert!(!stops.is_empty());
    assert!(
        stops
            .iter()
            .all(|c| c.contains(" stop --provider external --id moor-12")),
        "{stops:?}"
    );
    assert!(crabbox_calls(&rig)
        .iter()
        .any(|c| c.ends_with("list --provider external --json")));
}

#[test]
fn a_planned_vm_whose_stop_fails_for_another_reason_fails_the_boot() {
    let (rig, _vm) = vm_rig();
    let boxes = rig.state().join("boxes");
    std::fs::create_dir_all(&boxes).unwrap();
    let planned = json!({"ssh": "moor@minion", "slot": 2, "facts": {}, "prepared": false,
        "vm": {"provider": "external", "slug": "moor-7"}});
    std::fs::write(boxes.join("7.json"), planned.to_string()).unwrap();
    std::fs::write(rig.remote.join("stop-fails"), "permission denied").unwrap();
    let err = rig
        .call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap_err();
    assert!(err.contains("permission denied"), "{err}");
    assert_eq!(verbs(&rig), ["stop"]);
    std::fs::remove_file(rig.remote.join("stop-fails")).unwrap();
    std::fs::write(rig.remote.join("stop-gone"), "").unwrap();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST)
        .unwrap();
    assert_eq!(verbs(&rig), ["stop", "stop", "warmup", "ssh"]);
}

#[test]
fn the_vm_forward_runs_after_boot_and_ends_on_suspend_and_close() {
    let (rig, vm) = vm_rig();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST_PORTS)
        .unwrap();
    let forwards = vm.join("forwards");
    let up = "up -R 8081:127.0.0.1:8101".to_owned();
    eventually("the forward is up", || lines(&forwards) == [up.clone()]);
    suspend(&rig);
    let down = "down -R 8081:127.0.0.1:8101".to_owned();
    eventually("the forward ends on suspend", || {
        lines(&forwards) == [up.clone(), down.clone()]
    });
    rig.asleep.lock().unwrap().clear();
    rig.call_with("exec", json!({"command": "true"}), &VM_HOST_PORTS)
        .unwrap();
    eventually("the forward is up again", || lines(&forwards).len() == 3);
    rig.open.lock().unwrap().clear();
    eventually("the forward ends on close", || {
        lines(&forwards) == [up.clone(), down.clone(), up.clone(), down.clone()]
    });
}
