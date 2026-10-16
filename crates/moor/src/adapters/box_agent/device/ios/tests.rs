use std::path::PathBuf;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::LaunchReq;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::*;
use crate::testing::driver::FakeDriver;

const UDID: &str = "699BC2BD-E0B4-49A1-BFA1-3F7248BCD10D";
const CRED: &str = "/Users/box/.moor-cred-42-abc";

fn out() -> BoxPath {
    BoxPath::under(CRED, "simulator-udid").unwrap()
}

fn req(task: &str) -> LaunchReq {
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/r"),
        workspace: moor_core::seam::surface::WorkspaceRef::new("w"),
        task: TaskName::parse(task).unwrap(),
        task_id: TaskId::new(42),
        branch: "moor/x".to_owned(),
        goal: None,
        resume: None,
        issue: None,
        setup_exit: None,
        model: None,
        nonce: "abc".to_owned(),
        proxy: None,
        origin: None,
    }
}

fn driver(d: FakeDriver) -> FakeDriver {
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(42),
        name: TaskName::parse("42").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

#[test]
fn device_name_needs_ascii_digits() {
    assert_eq!(device_name("42").as_deref(), Some("moor-42"));
    for bad in ["", "t1", "4-2", "4 2", "٤٢", "42;x"] {
        assert_eq!(device_name(bad), None, "{bad:?}");
    }
}

#[test]
fn parse_udid_accepts_only_a_strict_uppercase_uuid() {
    assert_eq!(parse_udid(UDID).as_deref(), Some(UDID));
    assert_eq!(parse_udid(&format!(" {UDID}\n")).as_deref(), Some(UDID));
    for bad in [
        "booted",
        "all",
        "",
        "garbage",
        &UDID.to_lowercase(),
        &format!("{UDID} extra"),
        &format!("{UDID}\n{UDID}"),
        &format!("{UDID}-0"),
        "699BC2BD-E0B4-49A1-BFA1-3F7248BCD10",
        "699BC2BD-E0B4-49A1-BFA1-3F7248BCD10G",
    ] {
        assert_eq!(parse_udid(bad), None, "{bad:?}");
    }
}

#[test]
fn delete_line_refuses_anything_but_a_udid() {
    assert_eq!(
        Ios.teardown(UDID).as_deref(),
        Some(format!("xcrun simctl delete '{UDID}'").as_str())
    );
    for bad in ["booted", "all", "", "x'; rm -rf /", &format!("{UDID} all")] {
        assert_eq!(Ios.teardown(bad), None, "{bad:?}");
    }
}

#[test]
fn create_script_names_the_device_and_never_uses_booted() {
    let s = create_script("42", &out()).unwrap();
    assert!(s.contains("simctl create 'moor-42'"), "{s}");
    assert!(s.contains("command -v xcrun"), "{s}");
    assert!(s.contains(&format!("> '{CRED}/simulator-udid'")), "{s}");
    assert!(!s.contains("booted"), "{s}");
    assert_eq!(create_script("t1", &out()), None);
}

#[test]
fn create_returns_the_udid_the_script_wrote() {
    let d = driver(FakeDriver::new().peek_text("box-1", &out(), &format!("{UDID}\n")));
    let got = Ios.provision(&d, &req("42"), &BoxPath::restore(CRED).unwrap());
    assert_eq!(got.as_deref(), Ok(UDID));
    let execs = d.execs();
    assert_eq!(execs.len(), 1);
    let argv = execs.first().map(|e| e.1.argv.get(..2));
    assert_eq!(argv, Some(Some(&["sh".to_owned(), "-c".to_owned()][..])));
}

#[test]
fn create_fails_with_a_reason_when_exec_fails_or_exits_nonzero() {
    let cred = BoxPath::restore(CRED).unwrap();
    let d = driver(
        FakeDriver::new()
            .peek_text("box-1", &out(), UDID)
            .exec_results([Ok(ExecOutcome::Exited { code: 1 })]),
    );
    assert_eq!(
        Ios.provision(&d, &req("42"), &cred),
        Err("simctl create exited 1".to_owned())
    );
    let d = driver(
        FakeDriver::new()
            .peek_text("box-1", &out(), UDID)
            .exec_results([Err(DriverError::Failed {
                summary: "x".to_owned(),
            })]),
    );
    assert!(Ios.provision(&d, &req("42"), &cred).is_err());
}

#[test]
fn create_fails_with_a_reason_without_a_valid_device_file() {
    let cred = BoxPath::restore(CRED).unwrap();
    let d = driver(FakeDriver::new());
    assert_eq!(
        Ios.provision(&d, &req("42"), &cred),
        Err("no device file".to_owned())
    );
    let d = driver(FakeDriver::new().peek_text("box-1", &out(), "booted"));
    assert_eq!(
        Ios.provision(&d, &req("42"), &cred),
        Err("device file holds no UDID".to_owned())
    );
}

/// A named Task's simulator is named after its numeric id, which is what the release line deletes.
#[test]
fn a_named_task_gets_a_simulator_named_after_its_id() {
    let d = driver(FakeDriver::new().peek_text("box-1", &out(), &format!("{UDID}\n")));
    let got = Ios.provision(&d, &req("fix-login"), &BoxPath::restore(CRED).unwrap());
    assert_eq!(got.as_deref(), Ok(UDID));
    let execs = d.execs();
    let script = execs.first().and_then(|e| e.1.argv.get(2)).unwrap();
    assert!(
        script.contains("simctl create 'moor-42'") || script.contains("simctl create moor-42"),
        "{script}"
    );
    assert!(!script.contains("fix-login"), "{script}");
    assert!(Ios.release_line("42").contains("moor-42 ("));
}

const DEVICETYPES: &str = "\
== Device Types ==
iPad Pro (com.apple.CoreSimulator.SimDeviceType.iPad-Pro)
iPhone 17 Pro (com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro)
iPhone 16e (com.apple.CoreSimulator.SimDeviceType.iPhone-16e)
iPhone SE (3rd generation) (com.apple.CoreSimulator.SimDeviceType.iPhone-SE-3rd-generation)
iPhone Xʀ (com.apple.CoreSimulator.SimDeviceType.iPhone-XR)
iPhone 6s Plus (com.apple.CoreSimulator.SimDeviceType.iPhone-6s-Plus)
iPhone SE (1st generation) (com.apple.CoreSimulator.SimDeviceType.iPhone-SE)
";

const RUNTIMES: &str = "\
== Runtimes ==
iOS 18.5 - 18.5 (22F77) - com.apple.CoreSimulator.SimRuntime.iOS-18-5
iOS 26.4 - 26.4 (23E244) - com.apple.CoreSimulator.SimRuntime.iOS-26-4
";

/// Runs the script under `sh` with a fake `xcrun` that rejects the device types
/// named in `reject`; returns the `xcrun` calls and the UDID file, if written.
fn run_script(reject: &[&str]) -> (std::process::ExitStatus, String, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = dir.path().join("calls");
    let udid_file = dir.path().join("udid");
    let fake = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\n\
         case \"$2 $3\" in\n\
         'list devicetypes') cat <<'EOF'\n{DEVICETYPES}EOF\n;;\n\
         'list runtimes') cat <<'EOF'\n{RUNTIMES}EOF\n;;\n\
         create*) case \"$4\" in {reject}) echo 'code=403 Incompatible device' >&2; exit 1;; esac\n\
         echo {UDID};;\n\
         esac\n",
        log = log.display(),
        reject = if reject.is_empty() {
            "'-none-'".to_owned()
        } else {
            reject
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join("|")
        },
    );
    let path = bin.join("xcrun");
    std::fs::write(&path, fake).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = BoxPath::restore(udid_file.to_str().unwrap()).unwrap();
    let st = std::process::Command::new("sh")
        .arg("-c")
        .arg(create_script("42", &out).unwrap())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .status()
        .unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    (st, calls, std::fs::read_to_string(udid_file).ok())
}

const PREFIX: &str = "com.apple.CoreSimulator.SimDeviceType.";

#[test]
fn script_creates_with_the_newest_iphone_type_and_latest_runtime() {
    let (st, calls, udid) = run_script(&[]);
    assert!(st.success(), "{calls}");
    assert_eq!(udid.as_deref(), Some(format!("{UDID}\n").as_str()));
    let creates: Vec<&str> = calls.lines().filter(|l| l.contains("create")).collect();
    assert_eq!(
        creates,
        [format!("simctl create moor-42 {PREFIX}iPhone-17-Pro com.apple.CoreSimulator.SimRuntime.iOS-26-4")]
    );
    assert!(calls.contains(&format!("simctl boot {UDID}")), "{calls}");
}

#[test]
fn script_falls_through_to_the_next_type_when_one_is_rejected() {
    let (st, calls, udid) = run_script(&[&format!("{PREFIX}iPhone-17-Pro")]);
    assert!(st.success(), "{calls}");
    assert_eq!(udid.as_deref(), Some(format!("{UDID}\n").as_str()));
    let creates: Vec<&str> = calls.lines().filter(|l| l.contains("create")).collect();
    assert_eq!(creates.len(), 2, "{calls}");
    assert!(
        creates.first().is_some_and(|c| c.contains("iPhone-17-Pro")),
        "{calls}"
    );
    assert!(
        creates.get(1).is_some_and(|c| c.contains("iPhone-16e")),
        "{calls}"
    );
    assert!(!calls.contains("delete"), "{calls}");
}

#[test]
fn script_fails_without_writing_when_every_type_is_rejected() {
    let all: Vec<String> = [
        "iPhone-17-Pro",
        "iPhone-16e",
        "iPhone-SE-3rd-generation",
        "iPhone-XR",
        "iPhone-6s-Plus",
        "iPhone-SE",
    ]
    .iter()
    .map(|t| format!("{PREFIX}{t}"))
    .collect();
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let (st, calls, udid) = run_script(&refs);
    assert_eq!(st.code(), Some(1), "{calls}");
    assert_eq!(udid, None);
    assert_eq!(calls.matches("create").count(), 6, "{calls}");
    assert!(!calls.contains("boot"), "{calls}");
}

#[test]
fn brief_and_proof_make_the_simulator_paragraph() {
    let got = format!("{} {}\n\n", Ios.brief(UDID), Ios.proof(UDID));
    let want = format!(
        "This Task has its own iOS simulator, already booted: UDID `{UDID}`, also in \
         `$MOOR_SIM_UDID`. Use only this device: pass the UDID to `xcrun simctl` and \
         `xcodebuild -destination id={UDID}`; never use `booted` and never touch other \
         simulators. Put proof under `.moor/proof/` (git ignores it): \
         `xcrun simctl io \"$MOOR_SIM_UDID\" screenshot \"$PWD/.moor/proof/<name>.png\"`, \
         and for video `xcrun simctl io \"$MOOR_SIM_UDID\" recordVideo --codec=h264 \
         \"$PWD/.moor/proof/<name>.mp4\"`, stopped with Ctrl-C (SIGINT). simctl needs an \
         absolute output path, so build it from `$PWD` as shown.\n\n"
    );
    assert_eq!(got, want);
}

#[test]
fn env_and_grants_carry_a_valid_udid_only() {
    assert_eq!(
        Ios.env(UDID),
        vec![("MOOR_SIM_UDID".to_owned(), UDID.to_owned())]
    );
    assert_eq!(
        Ios.grants(UDID, "/Users/box"),
        vec![format!("/Users/box/{SIM_DEVICES_DIR}/{UDID}")]
    );
    for bad in ["", "booted", "../x", "699bc2bd-e0b4-49a1-bfa1-3f7248bcd10d"] {
        assert!(Ios.env(bad).is_empty(), "{bad:?}");
        assert!(Ios.grants(bad, "/Users/box").is_empty(), "{bad:?}");
    }
}

#[test]
fn available_only_on_darwin() {
    assert!(Ios.available("Darwin"));
    assert!(!Ios.available("Linux"));
}

#[test]
fn release_line_deletes_by_exact_name() {
    assert_eq!(
        Ios.release_line("42"),
        r#"command -v xcrun >/dev/null 2>&1 && xcrun simctl list devices 2>/dev/null | sed -n 's/^ *moor-42 (\([0-9A-Fa-f-]\{36\}\)).*/\1/p' | while read -r u; do xcrun simctl delete "$u" || true; done || true"#
    );
}
