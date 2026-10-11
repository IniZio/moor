use parity::adapter::{BoxState, EventKind, ExitClass, StartOpts, TaskState};
use parity::assert::{assert_never_called, assert_order, assert_seq_global};
use parity::fake::{claude, crabbox, herdr};
use parity::parity;
use parity::world::config::UserConfig;
use parity::world::repo::Repo;

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
