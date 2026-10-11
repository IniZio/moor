use std::path::Path;

use parity::adapter::{BoxState, EventKind, ExitClass, StartOpts, TaskState};
use parity::assert::{assert_never_called, assert_order, assert_seq_global};
use parity::fake::{claude, crabbox, herdr};
use parity::parity;
use parity::world::config::UserConfig;
use parity::world::repo::Repo;
use serde_json::json;

use super::support::*;
use crate::{moor, world};

parity!(
    start_list_close_host_agent,
    covers = ["A-02", "A-03", "A-04", "A-07", "A-15", "A-19", "REQ-47"],
    tags = [],
    {
        let w = world();
        let repo = Repo::create(&w, "widget");
        UserConfig::default().write(&w);
        crabbox::install(&w);
        herdr::install(&w);
        claude::install(&w);
        let name = w.task_name("demo");
        let m = moor();

        let opts = StartOpts {
            goal: Some("say hi".into()),
            ..Default::default()
        };
        let started = m.start(&w, &repo.path, &name, &opts);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let task = started.ok();
        assert_eq!(task.state, TaskState::Working);
        let boxed = task.boxed.as_ref().expect("box present");
        assert_eq!(boxed.state, BoxState::Ready);
        assert!(boxed.handle.is_some());

        let slug = crabbox::slug(&w).expect("warmup slug recorded");
        crabbox::inventory(&w, &[&slug]);

        let listed = m.list(&w, &repo.path);
        let tasks = listed.ok();
        let row = tasks.iter().find(|t| t.name == name).expect("task listed");
        assert_eq!(row.state, TaskState::Working);
        assert_eq!(row.boxed.as_ref().expect("box").state, BoxState::Ready);

        let closed = m.close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let view = closed.ok();
        assert!(view.box_deleted);
        assert_eq!(view.task.state, TaskState::Closed);
        assert_eq!(
            view.task.boxed.as_ref().expect("box").state,
            BoxState::Deleted
        );

        let after = m.list(&w, &repo.path);
        let after_tasks = after.ok();
        let row = after_tasks
            .iter()
            .find(|t| t.name == name)
            .expect("closed task still listed");
        assert_eq!(row.state, TaskState::Closed);
        assert_eq!(row.boxed.as_ref().expect("box").state, BoxState::Deleted);

        let kinds: Vec<EventKind> = m.events(&w, &name).into_iter().map(|e| e.kind).collect();
        let mut want = [
            EventKind::Started,
            EventKind::BoxReady,
            EventKind::SessionStarted,
            EventKind::Closed,
        ]
        .into_iter();
        let mut next = want.next();
        for k in &kinds {
            if Some(k) == next.as_ref() {
                next = want.next();
            }
        }
        assert!(
            next.is_none(),
            "event subsequence missing {next:?} in {kinds:?}"
        );

        let calls = w.all_calls();
        assert_order(
            &calls,
            &[
                ("herdr", &["worktree", "create"]),
                ("crabbox", &["warmup"]),
                ("crabbox", &["list"]),
                ("claude", &["agents"]),
                ("claude", &["stop"]),
                ("crabbox", &["stop"]),
                ("herdr", &["workspace", "close"]),
            ],
        );
        let bg: Vec<_> = calls
            .iter()
            .filter(|c| c.tool == "claude" && c.argv.iter().any(|a| a == "--bg"))
            .collect();
        assert_eq!(bg.len(), 1, "expected one claude --bg call");
        let list_seq = calls
            .iter()
            .find(|c| c.tool == "crabbox" && c.argv.first().map(String::as_str) == Some("list"))
            .expect("crabbox list")
            .seq;
        assert!(
            bg[0].seq < list_seq,
            "claude --bg must precede crabbox list"
        );

        let stop = calls
            .iter()
            .find(|c| c.tool == "crabbox" && c.argv.first().map(String::as_str) == Some("stop"))
            .expect("crabbox stop");
        assert_eq!(stop.flag("--id"), Some(slug));
        assert_eq!(
            stop.env
                .get("CRABBOX_NAMESPACE_DELETE_ON_RELEASE")
                .map(String::as_str),
            Some("1")
        );
        assert_seq_global(&calls);
        w.assert_no_new_tmp_creds();
    }
);

parity!(
    start_without_goal_dispatches_no_agent,
    covers = ["A-03", "A-19"],
    tags = [],
    {
        let w = world();
        let repo = Repo::create(&w, "widget");
        UserConfig::default().write(&w);
        crabbox::install(&w);
        herdr::install(&w);
        claude::install(&w);
        let name = w.task_name("quiet");
        let m = moor();

        let started = m.start(&w, &repo.path, &name, &StartOpts::default());
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        assert_eq!(started.ok().state, TaskState::Working);
        assert_never_called(&w.all_calls(), "claude", &["--bg"]);
        assert!(!w
            .calls("claude")
            .iter()
            .any(|c| c.argv.iter().any(|a| a == "--bg")));

        let closed = m.close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
    }
);

parity!(start_rejects_invalid_names, covers = ["A-01"], tags = [], {
    let f = fx();
    let long = "a".repeat(41);
    for bad in ["Bad_Name", "-lead", "has space", "", long.as_str()] {
        let o = f.try_start(bad, &StartOpts::default());
        assert!(o.class.is_err(), "{bad:?} accepted");
    }
    let calls = f.w.all_calls();
    assert_never_called(&calls, "herdr", &["worktree", "create"]);
    assert_never_called(&calls, "crabbox", &["warmup"]);
    assert!(f.list().is_empty());
});

parity!(
    capacity_queues_and_close_promotes_oldest,
    covers = ["A-05", "A-23", "A-50", "A-52", "REQ-20"],
    tags = [],
    {
        let f = fx_with(UserConfig::default().max_boxes(2));
        let (a, b) = (f.name("a"), f.name("b"));
        let (c, d) = (f.name("c"), f.name("d"));
        for n in [&a, &b] {
            let t = f.start(n);
            assert_eq!(box_state(&t), BoxState::Ready);
        }
        let queued = f.start(&c);
        assert_eq!(queued.state, TaskState::WaitingForBox);
        assert!(queued.boxed.is_none());
        assert!(queued.worktree.is_some() && queued.workspace_id.is_some());
        assert_eq!(f.start(&d).state, TaskState::WaitingForBox);
        assert_eq!(f.n("crabbox", "warmup"), 2);
        assert!(f.kinds(&c).contains(&EventKind::WaitingForBox));

        f.close(&a);
        f.sync_inventory();
        let rows = f.list();
        let find = |n: &str| rows.iter().find(|t| t.name == n).expect("listed").clone();
        assert_eq!(box_state(&find(&c)), BoxState::Ready);
        assert!(matches!(
            find(&c).state,
            TaskState::Working | TaskState::Idle
        ));
        assert_eq!(find(&d).state, TaskState::WaitingForBox);
        assert_eq!(f.n("crabbox", "warmup"), 3);
        assert_eq!(f.n("crabbox", "stop"), 1);
        assert_eq!(box_state(&find(&a)), BoxState::Deleted);
    }
);

parity!(
    gone_box_does_not_hold_capacity,
    covers = ["A-05", "REQ-16"],
    tags = [],
    {
        let f = fx_with(UserConfig::default().max_boxes(1));
        let (a, b) = (f.name("a"), f.name("b"));
        f.start(&a);
        crabbox::inventory(&f.w, &[]);
        let rows = f.list();
        let first = rows.iter().find(|t| t.name == a).expect("a listed");
        assert_eq!(box_state(first), BoxState::Gone);
        let second = f.start(&b);
        assert_ne!(second.state, TaskState::WaitingForBox);
        assert_eq!(box_state(&second), BoxState::Ready);
        assert_eq!(f.n("crabbox", "warmup"), 2);
    }
);

parity!(
    host_session_row_live_then_stopped_at_close,
    covers = ["A-06"],
    tags = [],
    {
        let f = fx();
        let name = f.name("sess");
        f.goal(&name);
        let before = f.m.sessions(&f.w, &name);
        assert_eq!(before.len(), 1);
        assert!(before[0].live && !before[0].box_pane);
        assert!(!before[0].session_id.is_empty());
        let closed = f.close(&name);
        assert_eq!(closed.sessions_stopped.len(), 1);
        let after = f.m.sessions(&f.w, &name);
        assert_eq!(after.len(), 1);
        assert!(!after[0].live);
        assert_eq!(after[0].session_id, before[0].session_id);
    }
);

parity!(
    state_dir_holds_registry_and_repo_stays_clean,
    covers = ["A-08"],
    tags = [],
    {
        let f = fx();
        let name = f.name("clean");
        f.goal(&name);
        let state = f.w.state_dir();
        assert!(state.join("registry.db").is_file());
        assert!(state.join("logs").is_dir());
        let status =
            f.w.command("git")
                .arg("-C")
                .arg(&f.repo.path)
                .args(["status", "--porcelain", "--ignored"])
                .output()
                .expect("git status");
        let text = String::from_utf8_lossy(&status.stdout).into_owned();
        assert!(text.trim().is_empty(), "repo not clean: {text}");
        assert!(!f.repo.path.join(".moor").exists());
        assert!(!f.repo.path.join(".crabbox").exists());
    }
);

parity!(
    start_fails_fast_when_herdr_missing,
    covers = ["A-10"],
    tags = [],
    {
        let f = fx();
        let herdr_bin = f.w.tool_path("herdr");
        std::fs::remove_file(&herdr_bin).expect("remove herdr link");
        let name = f.name("noherdr");
        let o = f.try_start(&name, &StartOpts::default());
        assert!(o.class.is_err());
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake"), &herdr_bin).expect("restore herdr");
        assert!(f.list().is_empty());
        assert_never_called(&f.w.all_calls(), "crabbox", &["warmup"]);
    }
);

parity!(
    start_rejects_non_repo_and_bad_worktree,
    covers = ["A-12"],
    tags = [],
    {
        let f = fx();
        let plain = f.w.root().join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir plain");
        let name = f.name("norepo");
        let o = f.m.start(&f.w, &plain, &name, &StartOpts::default());
        assert!(o.class.is_err());
        let main = StartOpts {
            worktree: Some(f.repo.path.clone()),
            ..Default::default()
        };
        assert!(f.try_start(&name, &main).class.is_err());
        let missing = StartOpts {
            worktree: Some(f.w.root().join("nope")),
            ..Default::default()
        };
        assert!(f.try_start(&name, &missing).class.is_err());
        let calls = f.w.all_calls();
        assert_never_called(&calls, "herdr", &["worktree", "create"]);
        assert_never_called(&calls, "crabbox", &["warmup"]);
        assert!(f.list().is_empty());
    }
);

parity!(
    unresolved_credential_ref_rejects_and_restart_makes_new_row,
    covers = ["A-13", "A-25"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "credentials": ["NOPE"] }));
        let name = f.name("rej");
        let o = f.try_start(&name, &StartOpts::default());
        assert!(o.class.is_err());
        let rows = f.rows(&name);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, TaskState::Rejected);
        assert!(rows[0].boxed.is_none());
        assert!(f.kinds(&name).contains(&EventKind::Rejected));
        let calls = f.w.all_calls();
        assert_never_called(&calls, "crabbox", &["warmup"]);
        assert_never_called(&calls, "herdr", &["worktree", "create"]);
        assert!(f.try_start(&name, &StartOpts::default()).class.is_err());
        let rows = f.rows(&name);
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert!(rows.iter().all(|t| t.state == TaskState::Rejected));
    }
);

parity!(
    start_unbound_records_head_as_base_and_creates_branch_worktree,
    covers = ["A-14"],
    tags = [],
    {
        let f = fx();
        let name = f.name("base");
        let t = f.start(&name);
        assert_eq!(t.base.as_deref(), Some(f.repo.head(&f.w).as_str()));
        let creates = herdr::calls_of(&f.w, "worktree", "create");
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].flag("--branch"), Some(format!("moor/{name}")));
        assert_eq!(f.start(&name).worktree, t.worktree);
        assert_eq!(herdr::calls_of(&f.w, "worktree", "create").len(), 1);
    }
);

parity!(
    start_bound_worktree_binds_branch_workspace_and_base,
    covers = ["A-14", "A-17", "A-44", "A-95", "REQ-41"],
    tags = [],
    {
        let f = fx();
        let wt = bound_worktree(&f, "feature/x");
        std::fs::write(wt.join("new.txt"), "new\n").unwrap();
        git_in(&f, &wt, &["add", "new.txt"]);
        git_in(&f, &wt, &["commit", "-q", "-m", "wt commit"]);
        let wt_head = git_in(&f, &wt, &["rev-parse", "HEAD"]);
        assert_ne!(wt_head, f.repo.head(&f.w));
        let name = f.name("mine");
        let t = f.start_with(&name, &bind_opts(&wt));
        assert_eq!(t.base.as_deref(), Some(wt_head.as_str()));
        assert_eq!(t.workspace_id.as_deref(), Some("ws-bound"));
        assert_eq!(herdr::calls_of(&f.w, "worktree", "create").len(), 0);
        let rename = herdr::calls_of(&f.w, "workspace", "rename");
        assert_eq!(rename.len(), 1);
        assert_eq!(rename[0].argv[2], "ws-bound");

        let elsewhere = bind_opts(Path::new("/"));
        assert!(f.try_start(&name, &elsewhere).class.is_err());

        let closed = f.close(&name);
        assert_eq!(closed.task.state, TaskState::Closed);
        assert_never_called(&f.w.all_calls(), "herdr", &["worktree", "remove"]);
        assert!(wt.is_dir());
        let branches = git_in(&f, &f.repo.path, &["branch", "--list", "feature/x"]);
        assert!(branches.contains("feature/x"));
    }
);

parity!(
    reopening_closed_bound_worktree_creates_workspace_and_new_row,
    covers = ["A-17", "A-25"],
    tags = [],
    {
        let f = fx();
        let wt = bound_worktree(&f, "feature/y");
        let first = f.name("first");
        let a = f.start_with(&first, &bind_opts(&wt));
        f.close(&first);
        herdr::worktrees(&f.w, &[("feature/y", wt.to_str().unwrap(), None)]);
        herdr::workspace_create(&f.w, "ws-new");
        let second = f.name("second");
        let b = f.start_with(&second, &bind_opts(&wt));
        assert_ne!(a.id, b.id);
        assert_ne!(b.state, TaskState::Closed);
        assert_eq!(b.workspace_id.as_deref(), Some("ws-new"));
        assert_eq!(f.row(&first).state, TaskState::Closed);
        let created = herdr::calls_of(&f.w, "workspace", "create");
        assert_eq!(created.len(), 1);
        assert_eq!(
            created[0].flag("--cwd").map(std::path::PathBuf::from),
            Some(wt.clone())
        );
        let rename = herdr::calls_of(&f.w, "workspace", "rename");
        assert_eq!(rename.last().unwrap().argv[2], "ws-new");
        assert_eq!(herdr::calls_of(&f.w, "worktree", "create").len(), 0);
    }
);

parity!(
    restart_of_closed_unbound_name_creates_new_task,
    covers = [],
    tags = ["new_only(RestartClosed)"],
    {
        let f = fx();
        let name = f.name("again");
        let first = f.start(&name);
        f.close(&name);
        let wt = first.worktree.clone().unwrap();
        herdr::worktrees(&f.w, &[(&format!("moor/{name}"), &wt, None)]);
        let second = f.start(&name);
        assert_ne!(second.id, first.id);
        assert_eq!(second.state, TaskState::Working);
    }
);

parity!(
    worktree_git_exclude_lists_crabbox_and_moor_dirs_once,
    covers = ["A-18", "REQ-25"],
    tags = [],
    {
        let f = fx();
        let name = f.name("excl");
        let t = f.start(&name);
        let wt = std::path::PathBuf::from(t.worktree.unwrap());
        f.start(&name);
        let rel = git_in(&f, &wt, &["rev-parse", "--git-path", "info/exclude"]);
        let path = if Path::new(&rel).is_absolute() {
            std::path::PathBuf::from(rel)
        } else {
            wt.join(rel)
        };
        let text = std::fs::read_to_string(&path).expect("read exclude");
        for dir in [".crabbox/", ".moor/"] {
            assert_eq!(
                text.lines().filter(|l| l.trim() == dir).count(),
                1,
                "{text}"
            );
        }
        assert!(text.lines().count() > 2, "existing lines preserved: {text}");
    }
);

parity!(
    start_evaluates_other_tasks_and_idles_agentless_ones,
    covers = ["A-11"],
    tags = [],
    {
        let f = fx();
        let (a, b) = (f.name("a"), f.name("b"));
        assert_eq!(f.start(&a).state, TaskState::Working);
        f.start(&b);
        assert_eq!(f.row(&a).state, TaskState::Idle);
        assert!(f.kinds(&a).contains(&EventKind::Idled));
    }
);

parity!(
    start_wakes_idle_task,
    covers = ["A-11", "A-43"],
    tags = [],
    {
        let f = fx();
        let (a, b) = (f.name("a"), f.name("b"));
        let first = f.start(&a);
        f.start(&b);
        assert_eq!(f.row(&a).state, TaskState::Idle);
        let again = f.start(&a);
        assert_eq!(again.id, first.id);
        assert_eq!(again.state, TaskState::Working);
        assert!(f.kinds(&a).contains(&EventKind::Busy));
        assert_eq!(f.n("crabbox", "warmup"), 2);
        assert_eq!(herdr::calls_of(&f.w, "worktree", "create").len(), 2);
    }
);

parity!(
    attach_on_waiting_task_reports_state_only,
    covers = ["A-43", "A-45"],
    tags = [],
    {
        let f = fx_with(UserConfig::default().max_boxes(1));
        let (a, b) = (f.name("a"), f.name("b"));
        f.start(&a);
        let queued = f.start(&b);
        assert_eq!(queued.state, TaskState::WaitingForBox);
        let again = f.start(&b);
        assert_eq!(again.id, queued.id);
        assert_eq!(again.state, TaskState::WaitingForBox);
        assert_eq!(f.n("crabbox", "warmup"), 1);
        assert_eq!(f.rows(&b).len(), 1);
    }
);
