//! The controller's own paths reach the macOS sandbox profile.

use super::*;
use crate::adapters::box_agent::nono_release::fake::{
    release_for, tarball, FakeFetch, READY_BINARY,
};
use moor_core::seam::driver::ExecOutcome;

#[test]
fn a_macos_profile_denies_the_controller_paths() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(READY_BINARY))));
    let release = release_for(&fetch, &tarball(READY_BINARY), ("Darwin", "arm64"));
    let d = driver(
        FakeDriver::new().exec_results([0, 0, 0].map(|code| Ok(ExecOutcome::Exited { code }))),
    );
    let uname = BoxPath::under(CRED, "uname").unwrap();
    d.set_peek_text("box-1", &uname, "Darwin arm64");
    let s = surface(FakeSurface::new("/r"));
    let paths = ["/Users/o/.local/state/moor", "/Users/o/.config/moor"];
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        Arc::new(release),
    )
    .with_moor_paths(paths.map(str::to_owned).to_vec())
    .launch(&req_with_proxy(&s))
    .unwrap();
    let put = (d.puts().into_iter()).find(|p| p.1.as_str().ends_with("nono-profile.json"));
    let doc: serde_json::Value = serde_json::from_slice(&put.unwrap().2).unwrap();
    let deny: Vec<&str> = (doc.pointer("/filesystem/deny").unwrap().as_array().unwrap())
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for p in paths {
        assert!(deny.contains(&p), "{p}: {deny:?}");
    }
}

#[path = "device_launch_tests.rs"]
mod device_launch_tests;
