//! A launch into a macOS Box layout takes the Box home from the Task and reads no `/proc`.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, BoxStep, Driver, ExecOutcome, StepDone};
use moor_core::seam::engine::{Engine, LaunchReq, MacGrants};
use moor_core::seam::surface::Surface;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::launch::install::{platform_step, probe_dir};
use crate::adapters::box_agent::launch::mac::Mac;
use crate::adapters::box_agent::nono_release::fake::ready_release;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const HOME: &str = "/Users/box/boxes/t1/home";
const LAUNCH: &str = "/Users/box/boxes/t1/launch";

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(d);
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

fn req(s: &FakeSurface) -> LaunchReq {
    let name = TaskName::parse("t1").unwrap();
    let wt = s
        .create_worktree(&PathBuf::from("/r"), &name, None)
        .unwrap();
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::restore("/Users/box/boxes/t1/work/repo").unwrap(),
        worktree: wt.path.clone(),
        workspace: s.ensure_workspace(&wt, "moor:t1").unwrap(),
        task: name,
        branch: "moor/t1".to_owned(),
        goal: Some("fix x".to_owned()),
        resume: None,
        issue: None,
        setup_exit: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        origin: None,
        strip_env: Vec::new(),
        mac_grants: MacGrants::default(),
    }
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        ready_release(),
    )
}

fn ran(code: i32, stdout: &str) -> StepDone {
    StepDone::Ran {
        outcome: ExecOutcome::Exited { code },
        stdout: stdout.to_owned(),
    }
}

#[test]
fn a_mac_launch_needs_no_process_environment_and_uses_the_task_home() {
    let d = driver(
        FakeDriver::new()
            .run_stdout("501\n/Users/alice\n/Applications/Xcode.app/Contents/Developer\n"),
    );
    let s = Arc::new(FakeSurface::new("/r"));
    let r = req(&s);
    agent(&d, &s).launch(&r).unwrap();
    let brief = d.puts().into_iter().next().unwrap().1;
    assert_eq!(
        brief.as_str(),
        format!("{LAUNCH}/.moor-cred-abc123/brief.md")
    );
}

#[test]
fn a_mac_launch_prepares_the_task_directories_before_anything_else() {
    let d = driver(
        FakeDriver::new()
            .run_stdout("501\n/Users/alice\n/Applications/Xcode.app/Contents/Developer\n"),
    );
    let s = Arc::new(FakeSurface::new("/r"));
    agent(&d, &s).launch(&req(&s)).unwrap();
    let execs = d.execs();
    let first = execs.first().expect("a prepare run").1.argv.join(" ");
    assert!(first.contains(LAUNCH), "{first}");
    assert!(first.contains("id -u"), "{first}");
    assert!(first.contains("\"$HOME\""), "{first}");
    assert!(
        first
            .trim_end()
            .ends_with(&format!("cat '{LAUNCH}/developer-dir'")),
        "{first}"
    );
}

#[test]
fn launch_dirs_on_a_mac_layout_come_from_the_task_home() {
    let d = driver(FakeDriver::new());
    let s = Arc::new(FakeSurface::new("/r"));
    let (cred, tmp) = agent(&d, &s).launch_dirs(&req(&s)).unwrap().unwrap();
    assert_eq!(cred.as_str(), format!("{LAUNCH}/.moor-cred-abc123"));
    assert_eq!(tmp.as_str(), "/Users/box/boxes/t1/tmp/.moor-tmp-abc123");
}

const PRINTED: &str = "501\n/Users/alice\n/Applications/Xcode-26.app/Contents/Developer\n";

fn mac() -> Mac {
    Mac::find(&req(&FakeSurface::new("/r"))).unwrap()
}

#[test]
fn the_mac_box_has_the_task_home_the_real_login_home_and_the_developer_dir() {
    let m = mac().user(Some(Ok(ran(0, PRINTED)))).ok().unwrap();
    assert_eq!(m.home().as_str(), HOME);
    assert!(!m.root);
    assert_eq!(m.login_home.as_str(), "/Users/alice");
    assert_eq!(
        m.developer_dir.as_str(),
        "/Applications/Xcode-26.app/Contents/Developer"
    );
}

#[test]
fn the_mac_user_is_root_only_when_the_prepare_step_prints_zero() {
    let out = PRINTED.replacen("501", "0", 1);
    assert!(mac().user(Some(Ok(ran(0, &out)))).ok().unwrap().root);
}

#[test]
fn a_relative_or_empty_login_home_fails_the_home_step() {
    for out in [
        "501\nalice\n/x/Developer\n",
        "501\n\n/x/Developer\n",
        "501\n",
    ] {
        let err = mac().user(Some(Ok(ran(0, out)))).err().unwrap();
        assert_eq!(err.step, "home", "{out:?}");
    }
}

#[test]
fn a_failed_prepare_step_reports_the_home_step() {
    let err = mac().user(Some(Ok(ran(3, "")))).err().unwrap();
    assert_eq!(err.step, "home");
    assert!(err.message.contains("exit 3"), "{}", err.message);
}

#[test]
fn the_platform_probe_of_a_mac_layout_reads_no_process_environment() {
    let s = FakeSurface::new("/r");
    let script = match platform_step(&probe_dir(&req(&s))) {
        BoxStep::Run(script) => script,
        other => format!("{other:?}"),
    };
    assert!(!script.contains("/proc"), "{script}");
    assert!(script.contains(LAUNCH), "{script}");
}

#[path = "mac_bin.rs"]
mod mac_bin;
