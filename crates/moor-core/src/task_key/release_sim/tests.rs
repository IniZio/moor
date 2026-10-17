//! Tests for the release scripts; behaviour runs under `sh` with stub `xcrun`, `mount` and `ps`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    delete_devices_script, failed_lines, remove_root_script, verify_report, verify_script,
    VerifyReport,
};
use crate::name::{BoxPath, TaskName};
use crate::task_key::release::{path_pids_cmd, tagged_pids_cmd};
use crate::task_key::{MacLayout, TaskKey};

const A: &str = "AAAAAAAA-1111-2222-3333-444444444444";
const B: &str = "BBBBBBBB-1111-2222-3333-444444444444";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moor-relsim-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn bp(&self, rel: &str) -> BoxPath {
        BoxPath::restore(self.path(rel).to_str().unwrap()).unwrap()
    }

    fn key() -> TaskKey {
        TaskKey::new(&TaskName::parse("t1").unwrap())
    }

    fn layout(&self) -> MacLayout {
        MacLayout::new(&self.bp("work"), &Self::key()).unwrap()
    }

    fn stub(&self, name: &str, body: &str) {
        let p = self.path(&format!("bin/{name}"));
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Runs `script` with the stubs first on `PATH`; returns (exit code, stdout).
    fn run(&self, script: &str) -> (Option<i32>, String) {
        self.run_home(script, None)
    }

    /// Like [`Self::run`] with `HOME` set to `home` (left as inherited when `None`).
    fn run_home(&self, script: &str, home: Option<&str>) -> (Option<i32>, String) {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script).env("PATH", path);
        if let Some(h) = home {
            cmd.env("HOME", h);
        }
        let out = cmd.output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An `xcrun` stub: `list` prints the devices in `$LIST`, other verbs append their args to calls.
fn fake_xcrun(s: &Scratch, list: &str, fail_delete_of: &str) {
    fs::write(s.path("list.txt"), list).unwrap();
    s.stub(
        "xcrun",
        &format!(
            "echo \"$*\" >> '{calls}'\n\
case \"$4\" in\n\
  (list) cat '{list}' ;;\n\
  (delete) [ \"$5\" = '{fail}' ] && {{ echo 'boom: cannot delete' >&2; exit 1; }}; exit 0 ;;\n\
  (*) exit 0 ;;\n\
esac",
            calls = s.path("calls.txt").display(),
            list = s.path("list.txt").display(),
            fail = fail_delete_of,
        ),
    );
}

fn calls(s: &Scratch) -> String {
    fs::read_to_string(s.path("calls.txt")).unwrap_or_default()
}

fn listing() -> String {
    format!(
        "== Devices ==\n-- iOS 17.0 --\n    iPhone 15 ({A}) (Shutdown)\n    iPad (10th generation) ({B}) (Booted)\n-- Unavailable: com.apple.CoreSimulator.SimRuntime.iOS-12-0 --\n    Old Phone (CCCCCCCC-1111-2222-3333-44444444444G) (Shutdown)\n    ({A}) not a device line\n"
    )
}

fn setup_logs(s: &Scratch, udids: &[&str]) -> String {
    for u in udids {
        let d = s.path(&format!("login/Library/Logs/CoreSimulator/{u}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("system.log"), "x").unwrap();
    }
    s.path("login").to_str().unwrap().to_owned()
}

/// A stateful `xcrun`: devices are files under `devs/` holding their state. `shutdown` marks one
/// Shutdown; `delete` fails for a Booted one; `list` prints the current devices.
fn stateful_xcrun(s: &Scratch, devices: &[(&str, &str)]) {
    fs::create_dir_all(s.path("devs")).unwrap();
    for (u, st) in devices {
        fs::write(s.path(&format!("devs/{u}")), st).unwrap();
    }
    let d = s.path("devs");
    s.stub(
        "xcrun",
        &format!(
            "echo \"$*\" >> '{calls}'\n\
case \"$4\" in\n\
  (list) echo '== Devices =='; for f in '{d}'/*; do [ -e \"$f\" ] && printf '    Phone (%s) (%s)\\012' \"$(basename \"$f\")\" \"$(cat \"$f\")\"; done; exit 0 ;;\n\
  (shutdown) [ -e '{d}'/\"$5\" ] && echo Shutdown > '{d}'/\"$5\"; exit 0 ;;\n\
  (delete) [ \"$(cat '{d}'/\"$5\")\" = Booted ] && {{ echo 'Unable to delete a Booted device' >&2; exit 1; }}; rm -f '{d}'/\"$5\"; exit 0 ;;\n\
esac",
            calls = s.path("calls.txt").display(),
            d = d.display(),
        ),
    );
}

fn make_sim_dir(l: &MacLayout) {
    fs::create_dir_all(l.sim.as_str()).unwrap();
}

#[test]
fn delete_script_text_is_scoped_to_the_task_set() {
    let s = Scratch::new("text");
    let script = delete_devices_script(&s.layout());
    assert!(script.contains("--set \"$sim\""));
    assert!(script.contains(&format!("sim='{}'", s.layout().sim.as_str())));
    assert!(script.contains("$HOME/Library/Logs/CoreSimulator"));
    assert!(!script.contains("shutdown booted"));
    assert!(!script.contains("delete all"));
}

#[test]
fn delete_removes_each_device_and_its_logs() {
    let s = Scratch::new("del");
    let l = s.layout();
    make_sim_dir(&l);
    fake_xcrun(&s, &listing(), "none");
    let home = setup_logs(&s, &[A, B, "KEEPME"]);
    let (code, out) = s.run_home(&delete_devices_script(&l), Some(&home));
    assert_eq!(code, Some(0), "{out}");
    let c = calls(&s);
    for u in [A, B] {
        assert!(c.contains(&format!("shutdown {u}")), "{c}");
        assert!(c.contains(&format!("delete {u}")), "{c}");
        assert!(!s
            .path(&format!("login/Library/Logs/CoreSimulator/{u}"))
            .exists());
    }
    assert!(
        !c.contains("CCCCCCCC"),
        "malformed udid must not match: {c}"
    );
    assert!(s.path("login/Library/Logs/CoreSimulator/KEEPME").exists());
    assert!(c.lines().all(|l| l.starts_with("simctl --set ")), "{c}");
}

#[test]
fn delete_continues_past_a_failure_and_exits_nonzero() {
    let s = Scratch::new("fail");
    let l = s.layout();
    make_sim_dir(&l);
    fake_xcrun(&s, &listing(), A);
    let home = setup_logs(&s, &[A, B]);
    let (code, out) = s.run_home(&delete_devices_script(&l), Some(&home));
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(failed_lines(&out).len(), 1, "{out}");
    assert!(
        out.contains(&format!("failed {A}: delete: boom: cannot delete")),
        "{out}"
    );
    assert!(calls(&s).contains(&format!("delete {B}")));
    assert!(!s
        .path(&format!("login/Library/Logs/CoreSimulator/{B}"))
        .exists());
}

#[test]
fn delete_with_missing_sim_dir_succeeds_without_calling_xcrun() {
    let s = Scratch::new("nosim");
    fake_xcrun(&s, &listing(), "none");
    let (code, out) = s.run(&delete_devices_script(&s.layout()));
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(calls(&s), "");
}

#[test]
fn delete_with_empty_set_succeeds() {
    let s = Scratch::new("empty");
    let l = s.layout();
    make_sim_dir(&l);
    fake_xcrun(&s, "== Devices ==\n", "none");
    let (code, _) = s.run(&delete_devices_script(&l));
    assert_eq!(code, Some(0));
}

#[test]
fn remove_root_deletes_the_tree() {
    let s = Scratch::new("rm");
    let l = s.layout();
    fs::create_dir_all(l.home.as_str()).unwrap();
    fs::write(
        format!("{}/f", l.work.as_str().rsplit_once('/').unwrap().0),
        "x",
    )
    .unwrap();
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(0), "{out}");
    assert!(!Path::new(l.root.as_str()).exists());
}

#[test]
fn remove_root_already_gone_is_success() {
    let s = Scratch::new("gone");
    let (code, out) = s.run(&remove_root_script(&s.layout()));
    assert_eq!((code, out.as_str()), (Some(0), ""));
}

#[test]
fn remove_root_reports_a_mount_under_home_and_does_not_unmount() {
    let s = Scratch::new("mnt");
    let l = s.layout();
    fs::create_dir_all(l.home.as_str()).unwrap();
    let mnt = format!("{}/vol", l.home.as_str());
    s.stub("rm", "echo 'rm: Operation not permitted' >&2; exit 1");
    s.stub(
        "mount",
        &format!(
            "echo '/dev/disk3s5 on /System/Volumes/Data (apfs, local)'\n\
echo 'map auto on {mnt} (autofs, nobrowse)'\n\
echo 'tmpfs on /elsewhere/vol type tmpfs (rw)'"
        ),
    );
    s.stub(
        "umount",
        &format!("echo called >> '{}'", s.path("umount.txt").display()),
    );
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(1));
    assert_eq!(out, format!("left: mount {mnt}\n"));
    assert!(!s.path("umount.txt").exists());
    assert!(!remove_root_script(&l).contains("umount"));
}

#[test]
fn remove_root_without_mount_reports_failed_root() {
    let s = Scratch::new("nomnt");
    let l = s.layout();
    fs::create_dir_all(l.home.as_str()).unwrap();
    s.stub("rm", "echo 'rm: denied' >&2; exit 1");
    s.stub("mount", "echo '/dev/x on / (apfs, local)'");
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(1));
    assert!(out.starts_with("failed root: rm: denied"), "{out}");
}

#[test]
fn delete_with_empty_or_relative_home_still_deletes_devices_and_reports_logs() {
    for home in ["", "rel/home"] {
        let s = Scratch::new("badhome");
        let l = s.layout();
        make_sim_dir(&l);
        stateful_xcrun(&s, &[(A, "Booted"), (B, "Shutdown")]);
        let (code, out) = s.run_home(&delete_devices_script(&l), Some(home));
        assert_eq!(code, Some(1), "{out}");
        assert_eq!(failed_lines(&out), ["logs: HOME is not an absolute path"]);
        assert!(!s.path(&format!("devs/{A}")).exists());
        assert!(!s.path(&format!("devs/{B}")).exists());
    }
}

#[test]
fn remove_root_shuts_down_a_booted_device_but_keeps_the_root_while_it_remains() {
    let s = Scratch::new("rmbooted");
    let l = s.layout();
    make_sim_dir(&l);
    stateful_xcrun(&s, &[(A, "Booted")]);
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(out, format!("left: device {A}\n"));
    assert!(calls(&s).contains(&format!("shutdown {A}")));
    assert!(!calls(&s).contains("shutdown booted"));
    assert!(Path::new(l.sim.as_str()).exists());
}

#[test]
fn remove_root_with_an_empty_set_removes_the_root() {
    let s = Scratch::new("rmempty");
    let l = s.layout();
    make_sim_dir(&l);
    stateful_xcrun(&s, &[]);
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(0), "{out}");
    assert!(!Path::new(l.root.as_str()).exists());
}

#[test]
fn booted_device_is_deleted_logs_gone_then_root_removed_in_order() {
    let s = Scratch::new("live");
    let l = s.layout();
    make_sim_dir(&l);
    stateful_xcrun(&s, &[(A, "Booted")]);
    let home = setup_logs(&s, &[A]);
    let (code, out) = s.run_home(&delete_devices_script(&l), Some(&home));
    assert_eq!(code, Some(0), "{out}");
    assert!(!s.path(&format!("devs/{A}")).exists());
    assert!(!s
        .path(&format!("login/Library/Logs/CoreSimulator/{A}"))
        .exists());
    let (code, out) = s.run(&remove_root_script(&l));
    assert_eq!(code, Some(0), "{out}");
    assert!(!Path::new(l.root.as_str()).exists());
}

fn ps_stub(s: &Scratch) {
    s.stub(
        "ps",
        "echo '  100     1 /usr/bin/ibtoold'\n\
echo '  200   300 /usr/bin/ibtoold'\n\
echo '  400     1 /usr/bin/other'\n\
echo '  500     1 ibtoold'",
    );
}

#[test]
fn verify_clean_exits_zero_and_still_warns_about_every_orphan_ibtoold() {
    let s = Scratch::new("vclean");
    ps_stub(&s);
    fake_xcrun(&s, "== Devices ==\n", "none");
    let (code, out) = s.run(&verify_script(&s.layout(), "true", "true"));
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(
        verify_report(&out),
        VerifyReport {
            left: vec![],
            warnings: vec!["ibtoold 100 ppid 1".into(), "ibtoold 500 ppid 1".into()],
        }
    );
}

#[test]
fn verify_reports_processes_root_and_devices() {
    let s = Scratch::new("vleft");
    let l = s.layout();
    make_sim_dir(&l);
    ps_stub(&s);
    fake_xcrun(&s, &listing(), "none");
    let (code, out) = s.run(&verify_script(&l, "echo 11; echo 22", "echo 22\necho 33"));
    assert_eq!(code, Some(1), "{out}");
    let r = verify_report(&out);
    assert_eq!(
        r.left,
        vec![
            "process 11".to_owned(),
            "process 22".into(),
            "process 33".into(),
            format!("root {}", l.root.as_str()),
            format!("device {A}"),
            format!("device {B}"),
        ]
    );
    assert_eq!(r.warnings.len(), 2);
}

#[test]
fn verify_snippet_failure_does_not_hide_the_other_snippet() {
    let s = Scratch::new("vsnip");
    ps_stub(&s);
    fake_xcrun(&s, "", "none");
    let (code, out) = s.run(&verify_script(&s.layout(), "exit 3", "echo 7"));
    assert_eq!(code, Some(1));
    assert_eq!(verify_report(&out).left, vec!["process 7".to_owned()]);
}

#[test]
fn verify_only_warning_is_not_left() {
    let s = Scratch::new("vwarn");
    ps_stub(&s);
    fake_xcrun(&s, "", "none");
    let (code, out) = s.run(&verify_script(&s.layout(), "", ""));
    assert_eq!(code, Some(0));
    assert!(verify_report(&out).left.is_empty());
}

#[test]
fn verify_report_ignores_other_lines() {
    let r = verify_report("noise\nleft: root /x\nwarn: ibtoold 1 ppid 1\nleft:x\n");
    assert_eq!(r.left, vec!["root /x".to_owned()]);
    assert_eq!(r.warnings, vec!["ibtoold 1 ppid 1".to_owned()]);
}

#[test]
fn failed_lines_keeps_text_after_prefix() {
    assert_eq!(
        failed_lines("failed U1: delete: x\nok\nfailed U2: remove logs: y\n"),
        vec!["U1: delete: x".to_owned(), "U2: remove logs: y".into()]
    );
}

/// The real verify script with the real matchers and the real `ps` reports nothing when nothing
/// of the Task exists, so the release's own subshells are never read as left processes.
#[test]
fn verify_real_matchers_report_nothing_when_nothing_exists() {
    let s = Scratch::new("vreal");
    let l = s.layout();
    let script = verify_script(&l, &tagged_pids_cmd(&Scratch::key()), &path_pids_cmd(&l));
    let calls = s.path("snapshots");
    for cols in ["pid=,ppid=,pgid=,sess=,args=", "pid=,ppid=,pgid=,args="] {
        let _ = fs::remove_file(&calls);
        let ps = format!(
            "moor_ps() {{ echo x >> '{}'; ps -e -ww -o {cols}; }}",
            calls.display()
        );
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("{ps}\n{script}"))
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            fs::read_to_string(&calls).unwrap_or_default(),
            "x\n",
            "{cols}: one snapshot"
        );
        assert!(!stdout.contains("left:"), "{cols}: {stdout}");
        assert_eq!(out.status.code(), Some(0), "{cols}: {stdout}");
    }
}

/// Every `case` pattern in a generated script, as written, for bash 3.2 inside `$( )`.
fn case_patterns(script: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_case = false;
    let mut want_pattern = false;
    for raw in script.lines() {
        let mut line = raw.trim();
        if !in_case {
            let Some(rest) = line.strip_prefix("case ") else {
                continue;
            };
            let Some((_, after)) = rest.split_once(" in") else {
                continue;
            };
            in_case = true;
            want_pattern = true;
            line = after.trim();
        }
        loop {
            if line.is_empty() {
                break;
            }
            if line.starts_with("esac") {
                in_case = false;
                break;
            }
            if want_pattern {
                found.push(line.to_owned());
                want_pattern = false;
            }
            match line.split_once(";;") {
                Some((_, after)) => {
                    want_pattern = true;
                    line = after.trim();
                }
                None => break,
            }
        }
        if !in_case {
            want_pattern = false;
        }
    }
    found
}

#[test]
fn case_patterns_in_generated_scripts_have_a_leading_paren() {
    let s = Scratch::new("casepat");
    let l = s.layout();
    let scripts = [
        ("delete", delete_devices_script(&l)),
        ("remove_root", remove_root_script(&l)),
        ("verify", verify_script(&l, "echo 1", "echo 2")),
    ];
    let mut total = 0;
    for (name, script) in &scripts {
        for pattern in case_patterns(script) {
            total += 1;
            assert!(
                pattern.starts_with('('),
                "{name}: case pattern needs a leading paren: {pattern}"
            );
        }
    }
    assert!(
        total >= 3,
        "expected case blocks in the scripts, saw {total}"
    );
}
