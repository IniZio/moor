//! A launch with the Android sandbox preset.

use super::*;

#[test]
fn the_android_preset_creates_its_directories_and_grants_kvm() {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new("/r")));
    let req = LaunchReq {
        sandbox: vec![moor_core::config::SandboxPreset::Android],
        ..req_with_proxy(&s)
    };
    agent(&d, &s).launch(&req).unwrap();
    let mkdir = d
        .execs()
        .into_iter()
        .filter_map(|e| e.1.argv.last().cloned())
        .find(|a| a.starts_with("mkdir -p"))
        .expect("mkdir step");
    assert!(mkdir.contains("/tmp/android-root"), "{mkdir}");
    let profile = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&profile.2).unwrap();
    assert_eq!(
        doc.pointer("/filesystem/allow_file/1"),
        Some(&serde_json::Value::from("/dev/kvm"))
    );
}

#[test]
fn a_failing_preset_mkdir_fails_the_sandbox_step_and_opens_no_pane() {
    let (d, s) = (
        driver(
            FakeDriver::new()
                .exec_results([0, 0, 0, 1].map(|code| Ok(ExecOutcome::Exited { code }))),
        ),
        surface(FakeSurface::new("/r")),
    );
    let req = LaunchReq {
        sandbox: vec![moor_core::config::SandboxPreset::Android],
        ..req_with_proxy(&s)
    };
    let message = sandbox_message(agent(&d, &s).launch(&req));
    assert!(message.contains("preset directories"), "{message}");
    assert!(s.opened().is_empty());
}
