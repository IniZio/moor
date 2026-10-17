//! A launch on a macOS Box in a Mac layout carries the repo's capabilities into the sandbox profile.

use super::*;
use crate::adapters::box_agent::nono_release::fake::{
    release_on, tarball, FakeFetch, READY_BINARY,
};
use moor_core::config::{BoxCapability, RepoSandbox};
use moor_core::seam::engine::MacGrants;
use moor_core::task_key::{MacLayout, TaskKey};

const PREPARED: &str = "501\n/Users/alice\n/Applications/Xcode.app/Contents/Developer\n";

/// The profile text a proxy launch uploads, from a Box of `platform` whose workdir is the `work/`
/// directory of a Mac layout and whose repo asks for `capabilities`.
fn layout_profile(platform: &str, capabilities: Vec<BoxCapability>) -> String {
    layout_launch(platform, capabilities, true).unwrap()
}

/// The profile text, or the launch failure's message, from a Box of `platform` whose repo asks for
/// `capabilities`; `in_layout` says whether the workdir is a Mac layout's `work/` directory.
fn layout_launch(
    platform: &str,
    capabilities: Vec<BoxCapability>,
    in_layout: bool,
) -> Result<String, String> {
    let (os, arch) = platform.split_once(' ').unwrap();
    let os: &'static str = if os == "Darwin" { "Darwin" } else { "Linux" };
    let arch: &'static str = if arch == "arm64" { "arm64" } else { "x86_64" };
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(READY_BINARY))));
    // A Mac layout workdir first runs the Task prepare step, which prints the uid, the login home
    // and the developer directory.
    let prepare = if in_layout { vec![PREPARED] } else { vec![] };
    let d = driver(FakeDriver::new().run_stdouts(prepare));
    d.set_run_stdout(platform);
    let s = Arc::new(FakeSurface::new("/r"));
    let task = TaskName::parse("t1").unwrap();
    let root = BoxPath::restore("/Users/box/moor").unwrap();
    let layout = MacLayout::new(&root, &TaskKey::new(&task)).unwrap();
    let r = LaunchReq {
        workdir: if in_layout { layout.work } else { root },
        mac_grants: MacGrants {
            capabilities,
            sandbox: RepoSandbox::default(),
        },
        ..req_with_proxy(&s)
    };
    let release = Arc::new(release_on((os, arch), &fetch, &tarball(READY_BINARY)));
    let a = BoxAgent::new(
        d.clone(),
        s,
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        release,
    );
    a.launch(&r).map_err(|e| e.to_string())?;
    let put = (d.puts().into_iter()).find(|p| p.1.as_str().ends_with("nono-profile.json"));
    Ok(String::from_utf8(put.unwrap().2).unwrap())
}

/// Whether the profile's JSON has a top-level key naming Seatbelt rules.
fn has_seatbelt_key(profile: &str) -> bool {
    let doc: serde_json::Value = serde_json::from_str(profile).unwrap();
    let keys = doc.as_object().unwrap().keys();
    keys.into_iter().any(|k| k.contains("seatbelt"))
}

#[test]
fn a_darwin_launch_in_a_mac_layout_gets_the_capability_rules() {
    let profile = layout_profile("Darwin arm64", vec![BoxCapability::Xcode]);
    assert!(has_seatbelt_key(&profile), "{profile}");
    assert!(profile.contains("com.apple.ibtool"), "{profile}");
    assert!(!profile.contains("/Users/box/moor/t1/sim"), "{profile}");
}

#[test]
fn the_simulator_directory_is_granted_only_for_ios_simulator() {
    let profile = layout_profile("Darwin arm64", vec![BoxCapability::IosSimulator]);
    let doc: serde_json::Value = serde_json::from_str(&profile).unwrap();
    let allow = doc.pointer("/filesystem/allow").unwrap().to_string();
    assert!(allow.contains("/Users/box/moor/t1/sim"), "{allow}");
}

#[test]
fn a_linux_launch_never_gets_the_mac_rules() {
    let profile = layout_profile("Linux x86_64", vec![]);
    assert!(!has_seatbelt_key(&profile), "{profile}");
    assert!(!profile.contains("com.apple.ibtool"), "{profile}");
}

#[test]
fn a_linux_launch_with_capabilities_fails() {
    let e = layout_launch("Linux x86_64", vec![BoxCapability::Xcode], true).unwrap_err();
    assert!(
        e.contains("box.capabilities applies only to a macOS Box"),
        "{e}"
    );
}

#[test]
fn a_darwin_launch_without_a_mac_layout_and_with_capabilities_fails() {
    let e = layout_launch("Darwin arm64", vec![BoxCapability::Xcode], false).unwrap_err();
    assert!(
        e.contains("box.capabilities applies only to a macOS Box"),
        "{e}"
    );
}
