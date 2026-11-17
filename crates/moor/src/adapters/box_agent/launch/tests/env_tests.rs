//! The agent runs without a sandbox: the script sets the agent's own environment.

use super::*;

/// The script a launch of `req` runs, on a Box whose login environment is `environ`.
fn script_for(environ: &str, req: impl FnOnce() -> LaunchReq) -> String {
    let d = driver(FakeDriver::new());
    d.set_peek_text("box-1", &environ_path(), environ);
    agent(&d).launch(&req()).unwrap();
    script_of(&d)
}

/// The position of the line that starts the agent.
fn agent_at(script: &str) -> usize {
    script.rfind("'claude'").expect(script)
}

const ROOT: &str = "PATH=/usr/bin\0HOME=/home/box\0USER=root\0";

#[test]
fn the_launch_script_never_runs_nono() {
    for script in [
        script_for(ENVIRON, req),
        script_for(ENVIRON, req_with_proxy),
        script_for(ROOT, req_with_proxy),
    ] {
        assert!(!script.contains("nono"), "{script}");
        assert!(!script.contains("unshare"), "{script}");
    }
}

#[test]
fn a_launch_puts_no_sandbox_tool_or_profile_on_the_box() {
    let d = driver(FakeDriver::new());
    agent(&d).launch(&req_with_proxy()).unwrap();
    for put in d.puts() {
        assert!(!put.1.as_str().contains("nono"), "{}", put.1.as_str());
    }
}

#[test]
fn a_root_agent_is_told_it_is_sandboxed() {
    for script in [script_for(ROOT, req), script_for(ROOT, req_with_proxy)] {
        let set = script.find("export IS_SANDBOX=1").expect(&script);
        assert!(set < agent_at(&script), "{script}");
    }
}

#[test]
fn a_non_root_agent_is_not_told_it_is_sandboxed() {
    for script in [
        script_for(ENVIRON, req),
        script_for(ENVIRON, req_with_proxy),
    ] {
        assert!(!script.contains("IS_SANDBOX"), "{script}");
    }
}

#[test]
fn repo_env_is_exported_for_the_agent() {
    let script = script_for(ENVIRON, || {
        let addr = ProxyAddr {
            keep_egress: true,
            box_env: vec![("APP_NOTE".to_owned(), "it's here".to_owned())],
            ..proxy_addr()
        };
        LaunchReq {
            profile: None,
            mcp: std::collections::BTreeMap::new(),
            proxy: Some(Arc::new(addr)),
            ..req_with_proxy()
        }
    });
    let set = script
        .find("export APP_NOTE='it'\\''s here'")
        .expect(&script);
    assert!(set < agent_at(&script), "{script}");
}
