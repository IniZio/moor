use parity::adapter::{BoxState, EventKind, ExitClass, TaskState};
use parity::assert::assert_never_called;
use parity::fake::{claude, crabbox, Rule};
use parity::parity;
use parity::world::config::UserConfig;
use serde_json::json;

use super::support::*;

parity!(
    close_reports_effects_and_second_close_fails,
    covers = ["A-90"],
    tags = [],
    {
        let f = fx();
        let name = f.name("closing");
        f.goal(&name);
        let view = f.close(&name);
        assert_eq!(view.task.state, TaskState::Closed);
        assert_eq!(view.sessions_stopped.len(), 1);
        assert!(view.box_deleted);
        assert!(view.workspace_closed);
        assert!(f.try_close(&name).class.is_err());
        assert_eq!(f.row(&name).state, TaskState::Closed);
    }
);

parity!(
    close_stops_agent_then_teardown_then_releases_box_then_closes_workspace,
    covers = ["A-91", "A-92"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "teardown": ["echo down-marker"] }));
        let name = f.name("order");
        f.goal(&name);
        f.close(&name);
        let calls = f.w.all_calls();
        let stop_agent = call_seq(&calls, |c| is_verb(c, "claude", "stop"));
        let teardown = call_seq(&calls, |c| {
            is_verb(c, "crabbox", "run") && c.argv.iter().any(|a| a.contains("down-marker"))
        });
        let release = call_seq(&calls, |c| is_verb(c, "crabbox", "stop"));
        let workspace = call_seq(&calls, |c| {
            c.tool == "herdr" && c.argv.starts_with(&["workspace".into(), "close".into()])
        });
        assert!(stop_agent < teardown && teardown < release && release < workspace);
        assert!(f.kinds(&name).contains(&EventKind::Closed));
    }
);

parity!(
    teardown_failure_never_blocks_close_and_absent_teardown_runs_nothing,
    covers = ["A-92", "REQ-43"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "teardown": ["echo down-marker"] }));
        crabbox::fail_match(&f.w, "run", "down-marker", 5);
        let name = f.name("tdfail");
        f.start(&name);
        let view = f.close(&name);
        assert!(view.box_deleted);
        assert_eq!(run_with(&f, "down-marker").len(), 1);
        assert_eq!(f.n("crabbox", "stop"), 1);

        let plain = fx();
        let other = plain.name("plain");
        plain.start(&other);
        plain.close(&other);
        assert!(run_with(&plain, "down-marker").is_empty());
    }
);

parity!(
    close_tolerates_stop_failure_when_agent_already_done,
    covers = ["A-91"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules("claude", [Rule::on(&["stop"]).exit(1).stderr("gone\n")]);
        claude::agents(
            &f.w,
            r#"[{"id":"a1","sessionId":"a1-full","cwd":"","state":"done"}]"#,
        );
        let name = f.name("done");
        f.goal(&name);
        let view = f.close(&name);
        assert_eq!(view.task.state, TaskState::Closed);
        assert_eq!(f.n("crabbox", "stop"), 1);
    }
);

parity!(
    close_keeps_checkout_branch_and_herdr_worktree,
    covers = ["A-93", "A-95", "REQ-14"],
    tags = [],
    {
        let f = fx();
        let name = f.name("keep");
        let t = f.start(&name);
        let wt = std::path::PathBuf::from(t.worktree.unwrap());
        std::fs::write(wt.join("work.txt"), "w").unwrap();
        git_in(&f, &wt, &["add", "work.txt"]);
        git_in(&f, &wt, &["commit", "-q", "-m", "unpushed"]);
        std::fs::write(wt.join("dirty.txt"), "d").unwrap();
        f.close(&name);
        assert!(wt.join("dirty.txt").exists());
        let branch = git_in(
            &f,
            &f.repo.path,
            &["branch", "--list", &format!("moor/{name}")],
        );
        assert!(branch.contains(&name));
        assert_never_called(&f.w.all_calls(), "herdr", &["worktree", "remove"]);
    }
);

parity!(
    close_refuses_unpushed_work_without_force,
    covers = ["A-93"],
    tags = ["new_only(CloseGate)"],
    {
        let f = fx();
        let name = f.name("gate");
        let t = f.start(&name);
        let wt = std::path::PathBuf::from(t.worktree.unwrap());
        std::fs::write(wt.join("work.txt"), "w").unwrap();
        git_in(&f, &wt, &["add", "work.txt"]);
        git_in(&f, &wt, &["commit", "-q", "-m", "unpushed"]);
        let refused = f.try_close(&name);
        assert!(refused.class.is_err(), "close must refuse unpushed work");
        assert_ne!(f.row(&name).state, TaskState::Closed);
        assert_eq!(f.n("crabbox", "stop"), 0);
        let forced = f.m.close(&f.w, &f.repo.path, &name, true);
        assert_eq!(forced.class, ExitClass::Ok, "{}", forced.stderr);
        assert_eq!(f.row(&name).state, TaskState::Closed);
    }
);

parity!(
    close_with_release_not_found_marks_box_deleted,
    covers = ["A-94"],
    tags = [],
    {
        let f = fx();
        crabbox::not_found(&f.w);
        let name = f.name("nf");
        f.start(&name);
        let view = f.close(&name);
        assert!(view.box_deleted);
        assert_eq!(box_state(&view.task), BoxState::Deleted);
    }
);

parity!(
    close_release_error_on_ready_box_keeps_task_open,
    covers = ["A-94"],
    tags = [],
    {
        let f = fx();
        let name = f.name("rel");
        f.goal(&name);
        crabbox::fail(&f.w, "stop");
        assert!(f.try_close(&name).class.is_err());
        let row = f.row(&name);
        assert_ne!(row.state, TaskState::Closed);
        assert_eq!(box_state(&row), BoxState::Ready);
        assert!(!f.m.sessions(&f.w, &name)[0].live);
    }
);

parity!(
    close_release_error_on_failed_box_warns_and_still_closes,
    covers = ["A-94", "A-97"],
    tags = [],
    {
        let f = fx();
        let name = f.name("fbox");
        failed_acquire(&f, &name);
        crabbox::fail(&f.w, "stop");
        let view = f.close(&name);
        assert_eq!(view.task.state, TaskState::Closed);
        assert_eq!(box_state(&view.task), BoxState::ReleaseFailed);
        assert!(view.task.boxed.unwrap().handle.is_some());
    }
);

parity!(
    close_of_waiting_task_releases_nothing,
    covers = ["A-97"],
    tags = [],
    {
        let f = fx_with(UserConfig::default().max_boxes(1));
        let (a, b) = (f.name("a"), f.name("b"));
        f.start(&a);
        f.start(&b);
        let view = f.close(&b);
        assert_eq!(view.task.state, TaskState::Closed);
        assert!(!view.box_deleted);
        assert_eq!(f.n("crabbox", "stop"), 0);
        assert!(view.workspace_closed);
    }
);
