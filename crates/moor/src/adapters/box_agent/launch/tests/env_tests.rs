//! The agent runs without a sandbox: the script clears the crabbox names and sets the agent's own.

use super::*;

/// The script a launch of `req` runs, on a Box whose login environment is `environ`.
fn script_for(environ: &str, req: impl FnOnce(&FakeSurface) -> LaunchReq) -> String {
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    d.set_peek_text("box-1", &environ_path(), environ);
    agent(&d, &s).launch(&req(&s)).unwrap();
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
    let (d, s) = (driver(FakeDriver::new()), surface(FakeSurface::new()));
    agent(&d, &s).launch(&req_with_proxy(&s)).unwrap();
    for put in d.puts() {
        assert!(!put.1.as_str().contains("nono"), "{}", put.1.as_str());
    }
}

#[test]
fn the_crabbox_gate_names_are_unset_before_the_agent_starts() {
    for script in [
        script_for(ENVIRON, req),
        script_for(ENVIRON, req_with_proxy),
    ] {
        let agent = agent_at(&script);
        let clear: Vec<&str> = script
            .lines()
            .filter(|l| l.contains("child_identity") || l.contains("NSC_"))
            .collect();
        let first = clear.first().expect(&script);
        assert!(script.find(first).unwrap() < agent, "{script}");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{}\nenv", clear.join("\n")))
            .env_clear()
            .envs([
                ("PATH", "/usr/bin:/bin"),
                ("child", "1"),
                ("child_identity", "1"),
                ("child_pid", "1"),
                ("command_umask", "1"),
                ("gate", "1"),
                ("run_dir", "1"),
                ("state", "1"),
                ("token", "1"),
                ("TOKEN", "1"),
                ("NSC_TOKEN", "1"),
                ("NSC_ENDPOINT", "1"),
                ("KEEP_ME", "1"),
                ("GH_TOKEN", "1"),
            ])
            .output()
            .unwrap();
        let env = String::from_utf8(out.stdout).unwrap();
        let names: Vec<&str> = env.lines().filter_map(|l| l.split('=').next()).collect();
        for gone in [
            "child",
            "child_identity",
            "child_pid",
            "command_umask",
            "gate",
            "run_dir",
            "state",
            "token",
            "TOKEN",
            "NSC_TOKEN",
            "NSC_ENDPOINT",
        ] {
            assert!(!names.contains(&gone), "{gone} survived: {env}");
        }
        assert!(names.contains(&"KEEP_ME"), "{env}");
        assert!(names.contains(&"GH_TOKEN"), "{env}");
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
    let script = script_for(ENVIRON, |s| {
        let addr = ProxyAddr {
            box_env: vec![("APP_NOTE".to_owned(), "it's here".to_owned())],
            ..proxy_addr()
        };
        LaunchReq {
            profile: None,
            mcp: std::collections::BTreeMap::new(),
            proxy: Some(Arc::new(addr)),
            ..req_with_proxy(s)
        }
    });
    let set = script
        .find("export APP_NOTE='it'\\''s here'")
        .expect(&script);
    assert!(set < agent_at(&script), "{script}");
}
