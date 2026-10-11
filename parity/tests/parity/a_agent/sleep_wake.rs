use parity::adapter::{BoxState, EventKind, ExitClass, TaskState};
use parity::assert::{assert_called, assert_never_called, assert_order};
use parity::fake::crabbox::{self, Preset};
use parity::parity;

use super::support::*;

parity!(
    host_agent_done_idles_and_busy_returns,
    covers = ["A-60", "A-61", "B-104"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        assert_eq!(fx.row().state, TaskState::Working);
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        assert!(fx.kinds().contains(&EventKind::Idled));
        agents(&fx.w, "working");
        assert_eq!(fx.row().state, TaskState::Working);
        assert!(fx.kinds().contains(&EventKind::Busy));
    }
);

parity!(
    start_pass_evaluates_other_tasks,
    covers = ["A-60", "B-82"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        assert!(!fx.kinds().contains(&EventKind::Idled));
        fx.start_named(&fx.w.task_name("second"), false);
        assert!(fx.kinds().contains(&EventKind::Idled));
    }
);

parity!(
    pane_and_wait_do_not_evaluate_idle,
    covers = ["A-60", "A-75"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        let p = fx.m.pane(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(p.ok().task.state, TaskState::Working);
        let wt = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(wt.class, ExitClass::Ok, "{}", wt.stderr);
        assert!(!fx.kinds().contains(&EventKind::Idled));
        assert_eq!(fx.row().state, TaskState::Idle);
    }
);

parity!(
    idle_task_sleeps_via_pause_after_age,
    covers = ["A-62", "B-103", "B-105"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        assert_eq!(fx.row().state, TaskState::Idle);
        assert!(fx.verb("pause").is_empty(), "no pause before the idle age");
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        let pauses = fx.verb("pause");
        assert_eq!(pauses.len(), 1);
        assert_eq!(
            pauses[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(pauses[0].flag("--id"), Some(fx.slug()));
        assert_eq!(fx.row().state, TaskState::Asleep);
        assert_eq!(fx.verb("pause").len(), 1);
    }
);

parity!(
    unsupported_provider_stays_up_and_reports_reason,
    covers = ["A-62", "A-70", "B-105", "REQ-45"],
    tags = [],
    {
        let fx = host_fx(Preset::LocalContainer);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Idle);
        assert_eq!(fx.box_state(&t), BoxState::Ready);
        assert!(t.sleep_reason().is_some_and(|r| !r.is_empty()));
        assert!(fx.verb("pause").is_empty());
        assert!(fx.row().sleep_reason().is_some());
    }
);

parity!(
    sprites_sleep_and_wake_make_no_driver_call,
    covers = ["A-62", "A-63", "B-89"],
    tags = [],
    {
        let fx = host_fx(Preset::Sprites);
        fx.asleep();
        assert!(fx.verb("pause").is_empty());
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(fx.verb("resume").is_empty());
        fx.assert_woken();
    }
);

parity!(
    steer_wakes_asleep_task_with_resume,
    covers = ["A-63", "A-80", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let resumes = fx.verb("resume");
        assert_eq!(resumes.len(), 1);
        assert_eq!(
            resumes[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(resumes[0].flag("--id"), Some(fx.slug()));
        fx.assert_woken();
        let calls = fx.w.all_calls();
        assert_order(
            &calls,
            &[
                ("crabbox", &["resume"]),
                ("claude", &["--resume", "a1-full"]),
            ],
        );
    }
);

parity!(
    open_wakes_asleep_task_and_reports_woke,
    covers = ["A-63", "A-76", "B-106", "B-107"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.open(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.ok().woke);
        assert_eq!(fx.verb("resume").len(), 1);
        assert_called(&fx.w.calls("herdr"), "herdr", &["workspace", "focus"]);
        fx.assert_woken();
        let again = fx.m.open(&fx.w, &fx.repo.path, &fx.name);
        assert!(!again.ok().woke);
        assert_eq!(fx.verb("resume").len(), 1);
        let missing = fx.m.open(&fx.w, &fx.repo.path, "no-such-task");
        assert!(missing.is_err());
    }
);

parity!(
    run_on_idle_task_marks_busy_by_run,
    covers = ["B-19"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        let o = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let busy: Vec<_> =
            fx.m.events(&fx.w, &fx.name)
                .into_iter()
                .filter(|e| e.kind == EventKind::Busy)
                .collect();
        assert_eq!(busy.len(), 1, "{busy:?}");
        assert_eq!(busy[0].payload["by"], "run");
        assert!(fx.verb("resume").is_empty());
    }
);

parity!(
    run_and_cp_wake_asleep_box,
    covers = ["A-63", "A-88", "A-89", "B-19", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["exit 3"],
            &Default::default(),
        );
        assert_eq!(o.class, ExitClass::Failed);
        assert_eq!(o.exit, 3);
        assert_eq!(fx.verb("resume").len(), 1);
        fx.assert_woken();
        let calls = fx.w.all_calls();
        assert_order(&calls, &[("crabbox", &["resume"]), ("crabbox", &["run"])]);
        assert!(fx.kinds().contains(&EventKind::Ran));

        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let dest = fx.w.root().join("copied.txt");
        let o =
            fx.m.cp(&fx.w, &fx.repo.path, &fx.name, "/tmp/out.txt", &dest);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(fx.verb("resume").len(), 1);
        assert_order(
            &fx.w.all_calls(),
            &[("crabbox", &["resume"]), ("crabbox", &["cp"])],
        );
        fx.assert_woken();
        assert!(fx.kinds().contains(&EventKind::Copied));
    }
);

parity!(
    failed_resume_keeps_task_asleep,
    covers = ["A-63", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        crabbox::fail(&fx.w, "resume");
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert!(o.is_err());
        let t = fx.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    gone_box_is_reconciled_and_never_woken,
    covers = ["A-64", "B-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let t = fx.row();
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        crabbox::fail(&fx.w, "list");
        assert_eq!(fx.box_state(&fx.row()), BoxState::Asleep);
        crabbox::inventory(&fx.w, &["somebody-else"]);
        let t = fx.row();
        assert_eq!(fx.box_state(&t), BoxState::Gone);
        assert!(fx.kinds().contains(&EventKind::BoxGone));
        let run = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert!(run.is_err());
        let cp = fx.m.cp(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            "/tmp/x",
            &fx.w.root().join("x"),
        );
        assert!(cp.is_err());
        let sh = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert!(sh.is_err());
        assert!(fx.verb("resume").is_empty());
        assert!(fx.verb("connect").is_empty());
    }
);

parity!(
    asleep_box_counts_toward_capacity,
    covers = ["B-107"],
    tags = [],
    {
        let fx = fixture(Preset::NamespaceDevbox, "host", Some(1));
        fx.asleep();
        let other = fx.start_named(&fx.w.task_name("queued"), false);
        assert_eq!(other.state, TaskState::WaitingForBox);
    }
);

parity!(
    shell_connects_to_ready_box_only,
    covers = ["A-77", "B-93"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        std::fs::write(fx.repo.path.join("crabbox.yaml"), "provider: x\n").unwrap();
        let task = fx.start(false);
        fx.present();
        let o = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let connects = fx.verb("connect");
        assert_eq!(connects.len(), 1);
        assert_eq!(
            connects[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(connects[0].flag("--id"), Some(fx.slug()));
        assert!(connects[0].argv.iter().any(|a| a == "--reclaim"));
        assert_eq!(
            std::fs::canonicalize(&connects[0].cwd).ok(),
            task.worktree
                .as_deref()
                .and_then(|p| std::fs::canonicalize(p).ok())
        );
        let cfg = connects[0]
            .env
            .get("CRABBOX_CONFIG")
            .expect("CRABBOX_CONFIG");
        assert!(cfg.ends_with("crabbox.yaml"), "{cfg}");
        crabbox::deny(&fx.w, "connect", 7);
        let failing = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(failing.exit, 7);
        let missing = fx.m.shell(&fx.w, &fx.repo.path, "no-such-task");
        assert!(missing.is_err());
    }
);

parity!(
    explicit_sleep_and_wake_commands,
    covers = ["REQ-09"],
    tags = ["new_only(ExplicitSleep)"],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let slept = fx.m.sleep_cmd(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(slept.ok().state, TaskState::Asleep);
        assert_eq!(fx.verb("pause").len(), 1);
        let woke = fx.m.wake_cmd(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(woke.ok().state, TaskState::Working);
        assert_eq!(fx.verb("resume").len(), 1);
    }
);
