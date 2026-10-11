use parity::adapter::{RunOpts, StartOpts, TaskState};
use parity::fake::{crabbox, Rule};
use parity::parity;
use parity::world::config::UserConfig;

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
