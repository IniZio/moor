//! The sandbox of a Box exec: the same profile as the agent's, on a macOS layout only.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::config::{BoxCapability, RepoSandbox};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq, MacGrants, SandboxReq, Sandboxed};
use moor_core::seam::surface::Surface;
use moor_core::{BoxHandle, BoxPath, ProviderId, Secret, TaskId, TaskName};

use super::super::claude::Claude;
use super::super::nono_release::fake::{release_on, tarball, FakeFetch, READY_BINARY};
use super::super::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const PREPARED: &str = "501\n/Users/alice\n/Applications/Xcode.app/Contents/Developer\n";
const CRED: &str = "/Users/box/boxes/t1/launch/.moor-cred-abc123";
const NOT_INSTALLED: i32 = 6;

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

/// A Box that prints `PREPARED` for the prepare step and `Darwin arm64` for the platform probe,
/// and answers each `Run` with the next of `codes`.
fn darwin_box(codes: &[i32]) -> Arc<FakeDriver> {
    let d = driver(
        FakeDriver::new()
            .run_stdouts([PREPARED])
            .exec_results(codes.iter().map(|&code| Ok(ExecOutcome::Exited { code }))),
    );
    d.set_run_stdout("Darwin arm64\n");
    d
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(READY_BINARY))));
    let release = release_on(("Darwin", "arm64"), &fetch, &tarball(READY_BINARY));
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        Arc::new(release),
    )
}

fn req(workdir: &str, report: bool) -> SandboxReq {
    SandboxReq {
        handle: BoxHandle::new("box-1"),
        worktree: PathBuf::from("/r/wt"),
        workdir: BoxPath::restore(workdir).unwrap(),
        task: TaskName::parse("t1").unwrap(),
        cred: BoxPath::restore(CRED).unwrap(),
        set_vars: vec![
            ("HOME".to_owned(), "/Users/box/boxes/t1/home".to_owned()),
            ("FOO".to_owned(), "bar".to_owned()),
        ],
        strip_env: Vec::new(),
        mac_grants: MacGrants::default(),
        report,
    }
}

fn mac_req() -> SandboxReq {
    req("/Users/box/boxes/t1/work/repo", false)
}

fn profile_put(d: &FakeDriver) -> Option<(BoxPath, Vec<u8>, u32)> {
    (d.puts().into_iter())
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .map(|p| (p.1, p.2, p.3))
}

fn message(out: Result<Option<Sandboxed>, EngineError>) -> String {
    match out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    }
}

#[test]
fn outside_a_mac_layout_there_is_no_sandbox_and_no_box_step() {
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let before = d.box_calls();
    let out = agent(&d, &s).sandbox(&req("/home/box/work", false));
    assert_eq!(out, Ok(None));
    assert_eq!(d.box_calls(), before);
    assert!(d.execs().is_empty() && d.puts().is_empty());
}

#[test]
fn a_mac_exec_gets_the_profile_in_its_cred_dir_with_its_own_environment() {
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let r = mac_req();
    agent(&d, &s).sandbox(&r).unwrap().expect("a sandbox");
    let (path, bytes, mode) = profile_put(&d).expect("a profile");
    assert_eq!(path.as_str(), format!("{CRED}/nono-profile.json"));
    assert_eq!(mode, 0o600);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let vars = doc.pointer("/environment/set_vars").expect("set_vars");
    assert_eq!(vars.get("HOME").unwrap(), "/Users/box/boxes/t1/home");
    assert_eq!(vars.get("FOO").unwrap(), "bar");
    let allow = doc.pointer("/environment/allow_vars").unwrap();
    assert!(allow.as_array().unwrap().contains(&"FOO".into()), "{doc}");
}

#[test]
fn the_start_is_the_login_home_nono_ending_in_the_separator() {
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let out = agent(&d, &s).sandbox(&mac_req()).unwrap().unwrap();
    let profile = format!("{CRED}/nono-profile.json");
    let want: Vec<String> = [
        "/Users/alice/.local/bin/nono",
        "-s",
        "run",
        "--profile",
        profile.as_str(),
        "--",
    ]
    .map(str::to_owned)
    .to_vec();
    assert_eq!(out.prefix, want);
    assert!(out.lines.iter().any(|l| l.contains("XDG_CONFIG_HOME")));
}

#[test]
fn a_report_adds_the_diagnostics_flag() {
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let r = req("/Users/box/boxes/t1/work/repo", true);
    let out = agent(&d, &s).sandbox(&r).unwrap().unwrap();
    assert_eq!(
        out.prefix.get(3).map(String::as_str),
        Some("--diagnostics-json")
    );
    assert_eq!(out.prefix.last().map(String::as_str), Some("--"));
    assert_eq!(out.prefix.len(), 7);
}

#[test]
fn a_missing_nono_is_uploaded_to_the_login_home_and_checked_again() {
    let d = darwin_box(&[0, 0, NOT_INSTALLED, 0]);
    let s = Arc::new(FakeSurface::new("/r"));
    agent(&d, &s).sandbox(&mac_req()).unwrap().unwrap();
    let puts: Vec<_> = d.puts().iter().map(|p| p.1.as_str().to_owned()).collect();
    assert!(
        puts.contains(&"/Users/alice/.local/bin/nono".to_owned()),
        "{puts:?}"
    );
    assert!(
        puts.contains(&format!("{CRED}/nono-profile.json")),
        "{puts:?}"
    );
    assert_eq!(d.execs().len(), 4);
}

#[test]
fn a_failed_check_names_the_sandbox_failure() {
    let d = darwin_box(&[0, 0, 5]);
    let s = Arc::new(FakeSurface::new("/r"));
    let text = message(agent(&d, &s).sandbox(&mac_req()));
    assert!(text.contains("Seatbelt is unavailable"), "{text}");
    assert!(profile_put(&d).is_none());
}

#[test]
fn a_nono_still_missing_after_the_upload_is_a_failure() {
    let d = darwin_box(&[0, 0, NOT_INSTALLED, NOT_INSTALLED]);
    let s = Arc::new(FakeSurface::new("/r"));
    let text = message(agent(&d, &s).sandbox(&mac_req()));
    assert!(
        text.contains("missing or is not the pinned binary"),
        "{text}"
    );
}

#[test]
fn an_unreadable_platform_is_a_failure() {
    let d = darwin_box(&[0, 3]);
    let s = Arc::new(FakeSurface::new("/r"));
    let text = message(agent(&d, &s).sandbox(&mac_req()));
    assert!(text.contains("Box platform"), "{text}");
}

/// The profile the agent launch writes for a Darwin Box in the same layout.
fn agent_profile(grants: &MacGrants) -> serde_json::Value {
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let name = TaskName::parse("t1").unwrap();
    let wt = s
        .create_worktree(&PathBuf::from("/r"), &name, None)
        .unwrap();
    let proxy = ProxyAddr {
        box_port: 4567,
        nonce: Secret::new(b"sentinel-9c2e".to_vec()),
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        box_env: Vec::new(),
        forge: false,
    };
    let launch = LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::restore("/Users/box/boxes/t1/work/repo").unwrap(),
        worktree: wt.path.clone(),
        workspace: s.ensure_workspace(&wt, "moor:t1").unwrap(),
        task: name,
        branch: "moor/t1".to_owned(),
        goal: None,
        resume: None,
        issue: None,
        setup_exit: None,
        model: None,
        nonce: "abc123".to_owned(),
        proxy: Some(Arc::new(proxy)),
        origin: None,
        strip_env: Vec::new(),
        mac_grants: grants.clone(),
    };
    agent(&d, &s).launch(&launch).unwrap();
    let (_, bytes, _) = profile_put(&d).expect("the agent profile");
    serde_json::from_slice(&bytes).unwrap()
}

const TMP: &str = "/Users/box/boxes/t1/tmp";

/// The profile without its temp write grants and its `allow_vars`, with the `allow_vars`.
///
/// The agent writes its own `.moor-tmp-<nonce>` inside the Task's temp directory; the exec has
/// only the Task's.
fn without_temp(doc: &serde_json::Value) -> (serde_json::Value, Vec<String>) {
    let mut doc = doc.clone();
    let allow = doc.pointer_mut("/filesystem/allow").unwrap();
    let allow = allow.as_array_mut().unwrap();
    let before = allow.len();
    allow.retain(|p| !p.as_str().is_some_and(|p| p.starts_with(TMP)));
    assert!(allow.len() < before, "a temp grant in {allow:?}");
    let vars = doc.pointer_mut("/environment").unwrap();
    let vars = vars.as_object_mut().unwrap().remove("allow_vars").unwrap();
    let vars = vars.as_array().unwrap().iter();
    let vars = vars.map(|v| v.as_str().unwrap().to_owned()).collect();
    (doc, vars)
}

fn non_empty(doc: &serde_json::Value, pointer: &str) {
    let found = doc.pointer(pointer).and_then(|v| v.as_array());
    assert!(found.is_some_and(|a| !a.is_empty()), "{pointer} in {doc}");
}

#[test]
fn the_exec_profile_is_the_one_the_agent_launch_writes() {
    let grants = MacGrants {
        capabilities: vec![
            BoxCapability::Xcode,
            BoxCapability::IosSimulator,
            BoxCapability::SwiftpmPlugins,
        ],
        sandbox: RepoSandbox {
            read: vec!["ios/Pods".to_owned()],
            write: vec!["build".to_owned()],
        },
        toolchains: Vec::new(),
        sim_udid: None,
    };
    let agent_doc = agent_profile(&grants);
    let set = agent_doc.pointer("/environment/set_vars").unwrap();
    let vars = (set.as_object().unwrap().iter())
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect();
    let d = darwin_box(&[]);
    let s = Arc::new(FakeSurface::new("/r"));
    let r = SandboxReq {
        set_vars: vars,
        mac_grants: grants,
        ..mac_req()
    };
    agent(&d, &s).sandbox(&r).unwrap().unwrap();
    let (_, bytes, _) = profile_put(&d).expect("a profile");
    let exec_doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for doc in [&agent_doc, &exec_doc] {
        non_empty(doc, &format!("/{}", super::super::sandbox::mac::RULES_KEY));
        non_empty(doc, "/environment/allow_vars");
        non_empty(doc, "/filesystem/deny");
    }
    let (exec_rest, exec_vars) = without_temp(&exec_doc);
    let (agent_rest, agent_vars) = without_temp(&agent_doc);
    assert_eq!(exec_rest, agent_rest);
    // Only the pass-through names differ: the agent also passes its own credential, and the exec
    // names every variable it sets.
    let only = |a: &[String], b: &[String]| -> Vec<String> {
        a.iter().filter(|v| !b.contains(v)).cloned().collect()
    };
    assert_eq!(
        only(&agent_vars, &exec_vars),
        ["CLAUDE_CODE_OAUTH_TOKEN".to_owned()]
    );
    let sets = exec_doc.pointer("/environment/set_vars").unwrap();
    for name in only(&exec_vars, &agent_vars) {
        assert!(sets.get(&name).is_some(), "{name} is not a set var");
    }
}
