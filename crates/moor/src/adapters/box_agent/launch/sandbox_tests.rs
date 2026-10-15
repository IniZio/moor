//! A launch with a Proxy runs the agent under the sandbox.

use super::*;
use moor_core::seam::driver::ExecOutcome;

#[test]
fn a_proxy_launch_wraps_the_agent_in_the_sandbox() {
    let (_, s) = launch_with_proxy();
    let script = script_of(&pane_line(&s));
    let nono = script.find("'nono' '-s' 'run' '--profile'").expect(&script);
    let port = script.find("'--open-port' '4567'").expect(&script);
    let agent = script.rfind("'claude'").expect(&script);
    assert!(nono < port && port < agent, "{script}");
}

#[test]
fn the_install_runs_before_the_hooks_are_put() {
    let (d, _) = launch_with_proxy();
    let calls = d.calls();
    let exec = calls.iter().position(|c| c.starts_with("exec")).unwrap();
    let hooks = calls.iter().position(|c| c.starts_with("put")).unwrap();
    assert!(exec < hooks, "{calls:?}");
    let script = d.execs().remove(0).1.argv.pop().unwrap();
    assert!(script.contains("sha256sum"), "{script}");
    assert!(script.contains("setup --check-only"), "{script}");
}

#[test]
fn the_profile_lands_in_the_cred_dir() {
    let (d, _) = launch_with_proxy();
    let put = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("nono-profile.json"));
    let (_, path, _, mode, _) = put.unwrap();
    assert_eq!(path.as_str(), format!("{CRED}/nono-profile.json"));
    assert_eq!(mode, 0o600);
}

#[test]
fn a_failed_install_fails_the_sandbox_step_and_opens_no_pane() {
    let (d, s) = (
        driver(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 5 })])),
        surface(FakeSurface::new("/r")),
    );
    let out = agent(&d, &s).launch(&req_with_proxy(&s));
    let message = match out {
        Err(EngineError::Failed {
            step: "sandbox",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    };
    assert!(message.contains("Landlock"), "{message}");
    assert!(s.opened().is_empty());
    assert_eq!(d.execs().pop().unwrap().1.argv, ["rm", "-rf", CRED]);
}

#[test]
fn a_launch_without_a_proxy_installs_nothing_and_stays_unsandboxed() {
    let (d, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    assert!(d.execs().is_empty());
    assert!(!pane_line(&s).contains("nono"));
}
