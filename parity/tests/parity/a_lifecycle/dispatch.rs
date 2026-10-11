use std::path::Path;

use parity::adapter::{ExitClass, RunOpts, StartOpts, TaskState};
use parity::fake::{crabbox, Rule};
use parity::parity;
use parity::world::config::UserConfig;
use serde_json::json;

use super::support::*;

parity!(
    goal_dispatches_once_and_issue_alone_never_dispatches,
    covers = ["A-35"],
    tags = [],
    {
        let f = fx();
        let issue = f.name("iss");
        let opts = StartOpts {
            issue: Some("ISSUE-1".into()),
            ..Default::default()
        };
        f.start_with(&issue, &opts);
        assert!(f.bg_calls().is_empty());
        assert!(f.m.sessions(&f.w, &issue).is_empty());

        let name = f.name("goal");
        f.goal(&name);
        assert_eq!(f.bg_calls().len(), 1);
        f.goal(&name);
        assert_eq!(f.bg_calls().len(), 1);
        assert_eq!(f.m.sessions(&f.w, &name).len(), 1);
    }
);

parity!(
    goal_added_to_running_goalless_task_dispatches_agent,
    covers = [],
    tags = ["new_only(GoalOnAttach)"],
    {
        let f = fx();
        let name = f.name("late");
        f.start(&name);
        f.goal(&name);
        assert_eq!(f.bg_calls().len(), 1);
        assert_eq!(f.m.sessions(&f.w, &name).len(), 1);
    }
);

parity!(
    host_agent_launch_runs_in_worktree_with_model_and_name,
    covers = ["A-37"],
    tags = [],
    {
        let f = fx();
        write_json(
            &f.w.config_path(),
            json!({ "mcp_servers": { "probe": { "command": "python3", "args": ["server.py"] } } }),
        );
        let name = f.name("launch");
        let opts = StartOpts {
            goal: Some("add x".into()),
            model: Some("sonnet".into()),
            ..Default::default()
        };
        let t = f.start_with(&name, &opts);
        let calls = f.bg_calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(
            std::fs::canonicalize(&call.cwd).ok(),
            std::fs::canonicalize(t.worktree.unwrap()).ok()
        );
        assert!(call.argv[0].contains("add x"), "brief carries the goal");
        assert_eq!(call.flag("--model").as_deref(), Some("sonnet"));
        assert_eq!(call.flag("--name").as_deref(), Some(name.as_str()));
        assert_eq!(
            call.flag("--permission-mode").as_deref(),
            Some("acceptEdits")
        );
        let mcp = call.flag("--mcp-config").expect("mcp config path");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&mcp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        f.close(&name);
        assert!(!Path::new(&mcp).exists(), "mcp file removed at close");
    }
);

parity!(
    host_dispatch_failure_leaves_task_working_and_start_retries,
    covers = ["A-38"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules(
            "claude",
            [Rule::any()
                .contains("--bg")
                .exit(1)
                .stderr("boom\n")
                .times(1)],
        );
        let name = f.name("dfail");
        let first = f.try_start(
            &name,
            &StartOpts {
                goal: Some("go".into()),
                ..Default::default()
            },
        );
        assert_eq!(first.class, ExitClass::Ok, "{}", first.stderr);
        assert_eq!(first.ok().state, TaskState::Working);
        assert!(f.m.sessions(&f.w, &name).is_empty());
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert_eq!(f.bg_calls().len(), 2);
        let sessions = f.m.sessions(&f.w, &name);
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].live);
    }
);

parity!(
    dead_provisioning_is_reported_not_attached,
    covers = ["A-49", "REQ-21"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules("crabbox", [Rule::on(&["warmup"]).block()]);
        let name = f.name("dead");
        let mut child =
            f.m.start_detached(&f.w, &f.repo.path, &name, &StartOpts::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while f.n("crabbox", "warmup") == 0 {
            assert!(std::time::Instant::now() < deadline, "warmup never called");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let pid = child.id() as i32;
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        child.wait().expect("reap start");
        assert_eq!(f.row(&name).state, TaskState::SettingUp);
        f.m.age(&f.w, &name, 600);
        let again = f.try_start(&name, &StartOpts::default());
        assert!(again.class.is_err(), "{}", again.stdout);
        assert_eq!(f.n("crabbox", "warmup"), 1);
    }
);

parity!(
    run_on_waiting_or_gone_box_task_fails_without_exec,
    covers = ["A-51"],
    tags = [],
    {
        let f = fx_with(UserConfig::default().max_boxes(1));
        let (a, b) = (f.name("a"), f.name("b"));
        f.start(&a);
        f.start(&b);
        let o = f.m.run(
            &f.w,
            &f.repo.path,
            &b,
            &["echo", "queued-marker"],
            &RunOpts::default(),
        );
        assert!(o.class.is_err());
        crabbox::inventory(&f.w, &[]);
        f.list();
        let o = f.m.run(
            &f.w,
            &f.repo.path,
            &a,
            &["echo", "gone-marker"],
            &RunOpts::default(),
        );
        assert!(o.class.is_err());
        assert!(run_with(&f, "queued-marker").is_empty());
        assert!(run_with(&f, "gone-marker").is_empty());
    }
);
