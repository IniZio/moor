//! A launch costs at most two Box calls after the connect lookup, and the nonce stays off argv.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq};
use moor_core::seam::surface::Surface;
use moor_core::{BoxHandle, BoxPath, ProviderId, Secret, TaskId, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::nono_release::fake::{
    ready_release, release_of, tarball, FakeFetch,
};
use crate::adapters::box_agent::nono_release::NonoRelease;
use crate::adapters::box_agent::sandbox::NOT_INSTALLED;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";
const CRED: &str = "/home/box/.moor-cred-t1-abc123";
const NONCE: &str = "distinct-nonce-7f3a91";

fn exited(codes: &[i32]) -> Vec<Result<ExecOutcome, DriverError>> {
    codes
        .iter()
        .map(|&code| Ok(ExecOutcome::Exited { code }))
        .collect()
}

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let d = Arc::new(
        d.peek_text("box-1", &environ, ENVIRON)
            .run_stdout("Linux x86_64"),
    );
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

fn req(s: &FakeSurface, proxy: bool) -> LaunchReq {
    let name = TaskName::parse("t1").unwrap();
    let wt = s
        .create_worktree(&PathBuf::from("/r"), &name, None)
        .unwrap();
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
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
        proxy: proxy.then(|| {
            Arc::new(ProxyAddr {
                box_port: 4567,
                nonce: Secret::new(NONCE.as_bytes().to_vec()),
                ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
                box_env: Vec::new(),
                forge: false,
            })
        }),
        origin: proxy.then(|| "git@github.com:o/n.git".to_owned()),
        strip_env: Vec::new(),
        sandbox: Vec::new(),
    }
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>, release: Arc<NonoRelease>) -> BoxAgent {
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        release,
    )
}

struct Run {
    d: Arc<FakeDriver>,
    s: Arc<FakeSurface>,
    out: Result<(), EngineError>,
}

fn launch(proxy: bool, check_codes: &[i32], release: Arc<NonoRelease>) -> Run {
    let d = driver(FakeDriver::new().exec_results(exited(check_codes)));
    let s = Arc::new(FakeSurface::new("/r"));
    let out = agent(&d, &s, release).launch(&req(&s, proxy)).map(|_| ());
    Run { d, s, out }
}

fn missing_nono_release() -> Arc<NonoRelease> {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"the binary"))));
    Arc::new(release_of(&fetch, &tarball(b"the binary")))
}

fn box_calls(d: &FakeDriver) -> Vec<String> {
    let mut calls = d.box_calls();
    assert_eq!(calls.remove(0), "acquire");
    calls
}

fn assert_connect_argv_reused(d: &FakeDriver) {
    let connects = d.batch_connects();
    assert!(connects.iter().all(|c| !c.is_empty()), "{connects:?}");
    assert!(connects.iter().all(|c| Some(c) == connects.first()));
}

#[test]
fn a_warm_proxy_launch_makes_two_batch_calls_after_the_connect_lookup() {
    let run = launch(true, &[0, 0], ready_release());
    run.out.unwrap();
    assert_eq!(box_calls(&run.d), ["connect_argv", "batch", "batch"]);
    assert_eq!(run.d.batch_connects().len(), 2);
    assert_connect_argv_reused(&run.d);
}

#[test]
fn a_launch_without_a_proxy_makes_two_batch_calls_after_the_connect_lookup() {
    let run = launch(false, &[], ready_release());
    run.out.unwrap();
    assert_eq!(box_calls(&run.d), ["connect_argv", "batch", "batch"]);
    assert_connect_argv_reused(&run.d);
}

#[test]
fn a_missing_nono_adds_only_the_upload_and_the_recheck_to_the_third_batch() {
    let run = launch(true, &[0, 0, NOT_INSTALLED, 0], missing_nono_release());
    run.out.unwrap();
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "batch"]
    );
    assert_connect_argv_reused(&run.d);
    let puts = run.d.puts();
    let first = puts.first().expect("a put");
    assert_eq!(first.1.as_str(), "/home/box/.local/bin/nono");
    assert_eq!(first.2, b"the binary");
    assert_eq!(first.3, 0o755);
    let execs = run.d.execs();
    let scripts: Vec<_> = execs.iter().filter_map(|e| e.1.argv.get(2)).collect();
    assert_eq!(scripts.len(), 4);
    assert_eq!(scripts.get(3), scripts.get(2));
}

#[test]
fn a_failed_recheck_refuses_the_launch_like_a_failed_first_check() {
    let run = launch(true, &[0, 0, NOT_INSTALLED, 5], missing_nono_release());
    let message = match run.out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    };
    assert!(message.contains("Landlock"), "{message}");
    assert!(run.s.opened().is_empty());
}

#[test]
fn a_failed_nono_upload_is_a_sandbox_failure_and_stops_the_batch() {
    let failing = FakeDriver::new()
        .exec_results(exited(&[0, 0, NOT_INSTALLED]))
        .fail_put(DriverError::Quota("upload broke".to_owned()));
    let d = driver(failing);
    let s = Arc::new(FakeSurface::new("/r"));
    let out = agent(&d, &s, missing_nono_release()).launch(&req(&s, true));
    match out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => assert!(message.contains("upload broke"), "{message}"),
        other => unreachable!("unexpected {other:?}"),
    }
    let calls = box_calls(&d);
    let launch_calls = calls.iter().take(4).map(String::as_str);
    assert!(launch_calls.eq(["connect_argv", "batch", "batch", "batch"]));
}

#[test]
fn a_failed_check_refuses_the_launch_before_anything_is_uploaded() {
    let run = launch(true, &[0, 0, 5], ready_release());
    let message = match run.out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    };
    assert!(message.contains("Landlock"), "{message}");
    assert!(run.d.puts().is_empty());
    assert!(run.s.opened().is_empty());
}

fn agent_failure(run: Run) -> String {
    match run.out {
        Err(EngineError::Failed {
            step: "agent",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    }
}

#[test]
fn a_missing_agent_refuses_a_proxy_launch_before_anything_is_uploaded() {
    let run = launch(true, &[0, 1], ready_release());
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "exec"]
    );
    assert!(run.d.puts().is_empty());
    assert!(run.s.opened().is_empty());
    let message = agent_failure(run);
    assert!(message.contains("`claude`"), "{message}");
    assert!(message.contains("`setup`"), "{message}");
}

#[test]
fn a_missing_agent_refuses_a_launch_without_a_proxy_before_anything_is_uploaded() {
    let run = launch(false, &[127], ready_release());
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "exec"]
    );
    assert!(run.d.puts().is_empty());
    assert!(agent_failure(run).contains("not on the Box's PATH"));
}

#[test]
fn the_agent_check_names_the_program_and_the_launch_path() {
    let run = launch(false, &[], ready_release());
    run.out.unwrap();
    let execs = run.d.execs();
    let script = execs
        .iter()
        .filter_map(|e| e.1.argv.get(2))
        .find(|a| a.contains("command -v"))
        .expect("agent check");
    assert_eq!(
        script,
        "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"; command -v 'claude' >/dev/null 2>&1"
    );
}

fn leaks(text: &str) -> bool {
    text.contains(NONCE)
}

#[test]
fn the_nonce_reaches_the_box_only_through_its_staged_file() {
    let run = launch(true, &[0, 0], ready_release());
    run.out.unwrap();
    let (d, s) = (&run.d, &run.s);
    assert!(d.execs().iter().all(|e| !leaks(&format!("{e:?}"))));
    assert!(d.calls().iter().all(|c| !leaks(c)));
    assert!(box_calls(d).iter().all(|c| !leaks(c)));
    assert!(d
        .batch_connects()
        .iter()
        .all(|c| c.iter().all(|a| !leaks(a))));
    assert!(s.opened().iter().all(|c| !leaks(&format!("{c:?}"))));
    assert!(s.calls().iter().all(|c| !leaks(c)));
    let holders: Vec<_> = d
        .puts()
        .into_iter()
        .filter(|p| leaks(&String::from_utf8_lossy(&p.2)))
        .map(|p| (p.1.as_str().to_owned(), p.3))
        .collect();
    assert_eq!(holders, [(format!("{CRED}/proxy-nonce"), 0o600)]);
}

const GUARD: &str = "[ -r /proc/self/environ ] && [ -n \"$HOME\" ] || exit 1;";

#[test]
fn the_platform_script_does_nothing_unless_the_box_has_a_home() {
    let run = launch(true, &[0, 0], ready_release());
    run.out.unwrap();
    let execs = run.d.execs();
    let script = execs
        .iter()
        .flat_map(|e| e.1.argv.iter())
        .find(|a| a.contains("mkdir"))
        .expect("platform script");
    assert!(script.starts_with(GUARD), "{script}");
}

#[test]
fn a_launch_with_no_environ_reports_the_home_step() {
    let d = Arc::new(FakeDriver::new().run_stdout("Linux x86_64"));
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    let s = Arc::new(FakeSurface::new("/r"));
    let out = agent(&d, &s, ready_release()).launch(&req(&s, true));
    let got = match out {
        Err(EngineError::Failed { step, message }) => format!("{step}: {message}"),
        other => format!("unexpected {other:?}"),
    };
    assert_eq!(got, "home: the Box has no process environment");
}

mod gh;
