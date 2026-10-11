use std::path::Path;

use parity::adapter::{
    BoxState, CloseView, EventKind, ExitClass, Moor, Outcome, RunOpts, StartOpts, TaskState,
    TaskView,
};
use parity::assert::{assert_never_called, assert_order, assert_pure_json, assert_seq_global};
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::parity;
use parity::world::config::UserConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

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

struct Fx {
    w: World,
    repo: Repo,
    m: Box<dyn Moor>,
}

fn fx_with(user: UserConfig) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    user.write(&w);
    crabbox::install(&w);
    herdr::install(&w);
    claude::install(&w);
    follow_warmups(&w);
    Fx { w, repo, m: moor() }
}

fn follow_warmups(w: &World) {
    let script = concat!(
        "printf '['; sep=''; ",
        "for s in $(grep -o '\"--slug\",\"[^\"]*\"' {root}/fakes/crabbox.jsonl | cut -d'\"' -f4); do ",
        "printf '%s{\"name\":\"%s\"}' \"$sep\" \"$s\"; sep=','; done; printf ']'"
    );
    w.prepend_rules("crabbox", [Rule::on(&["list"]).exec(script)]);
}

fn fx() -> Fx {
    fx_with(UserConfig::default())
}

fn write_json(path: &Path, v: Value) {
    std::fs::write(path, v.to_string()).expect("write config");
}

impl Fx {
    fn name(&self, base: &str) -> String {
        self.w.task_name(base)
    }
    fn try_start(&self, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
        self.m.start(&self.w, &self.repo.path, name, opts)
    }
    fn start_with(&self, name: &str, opts: &StartOpts) -> TaskView {
        let o = self.try_start(name, opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    fn start(&self, name: &str) -> TaskView {
        self.start_with(name, &StartOpts::default())
    }
    fn goal(&self, name: &str) -> TaskView {
        let opts = StartOpts {
            goal: Some("say hi".into()),
            ..Default::default()
        };
        self.start_with(name, &opts)
    }
    fn list(&self) -> Vec<TaskView> {
        let o = self.m.list(&self.w, &self.repo.path);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    fn rows(&self, name: &str) -> Vec<TaskView> {
        self.list().into_iter().filter(|t| t.name == name).collect()
    }
    fn row(&self, name: &str) -> TaskView {
        let mut rows = self.rows(name);
        assert!(!rows.is_empty(), "task {name} not listed");
        rows.remove(rows.len() - 1)
    }
    fn try_close(&self, name: &str) -> Outcome<CloseView> {
        self.m.close(&self.w, &self.repo.path, name, false)
    }
    fn close(&self, name: &str) -> CloseView {
        let o = self.try_close(name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    fn kinds(&self, name: &str) -> Vec<EventKind> {
        self.m
            .events(&self.w, name)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }
    fn verb(&self, tool: &str, verb: &str) -> Vec<Call> {
        self.w
            .calls(tool)
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
            .collect()
    }
    fn n(&self, tool: &str, verb: &str) -> usize {
        self.verb(tool, verb).len()
    }
    fn repo_cfg(&self, v: Value) {
        write_json(&self.repo.path.join(".moor.yaml"), v);
    }
    fn sync_inventory(&self) {
        let slugs: Vec<String> = self
            .verb("crabbox", "warmup")
            .iter()
            .filter_map(|c| c.flag("--slug"))
            .collect();
        let refs: Vec<&str> = slugs.iter().map(String::as_str).collect();
        crabbox::inventory(&self.w, &refs);
    }
    fn bg_calls(&self) -> Vec<Call> {
        self.w
            .calls("claude")
            .into_iter()
            .filter(|c| c.argv.iter().any(|a| a == "--bg"))
            .collect()
    }
}

fn box_state(t: &TaskView) -> BoxState {
    t.boxed.as_ref().expect("box present").state.clone()
}

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
    covers = ["A-05", "A-23", "A-50", "A-52"],
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
    covers = ["A-05"],
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

fn git_in(f: &Fx, dir: &Path, args: &[&str]) -> String {
    let o =
        f.w.command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn bound_worktree(f: &Fx, branch: &str) -> std::path::PathBuf {
    let wt = f.w.root().join("trees").join(branch.replace('/', "-"));
    git_in(
        f,
        &f.repo.path,
        &["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()],
    );
    let wt = std::fs::canonicalize(&wt).expect("canonical worktree");
    herdr::worktrees(&f.w, &[(branch, wt.to_str().unwrap(), Some("ws-bound"))]);
    wt
}

fn bind_opts(wt: &Path) -> StartOpts {
    StartOpts {
        worktree: Some(wt.to_path_buf()),
        ..Default::default()
    }
}

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
    covers = ["A-14", "A-17", "A-44", "A-95"],
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
    covers = ["A-18"],
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

fn fx_preset(preset: crabbox::Preset, user: UserConfig) -> Fx {
    let f = fx_with(user.provider(preset.provider()));
    crabbox::install_preset(&f.w, preset);
    f
}

fn call_seq(calls: &[Call], pick: impl Fn(&Call) -> bool) -> u64 {
    calls
        .iter()
        .find(|c| pick(c))
        .map_or_else(|| panic!("call missing"), |c| c.seq)
}

fn is_verb(c: &Call, tool: &str, verb: &str) -> bool {
    c.tool == tool && c.argv.first().map(String::as_str) == Some(verb)
}

fn is_bg(c: &Call) -> bool {
    c.tool == "claude" && c.argv.iter().any(|a| a == "--bg")
}

parity!(
    box_credential_preflight_failure_acquires_no_box,
    covers = ["A-20"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "agent": { "location": "box" } }));
        git_in(
            &f,
            &f.repo.path,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                "https://github.com/acme/gadget",
            ],
        );
        let name = f.name("pre");
        let o = f.try_start(&name, &StartOpts::default());
        assert!(o.class.is_err());
        assert_eq!(f.row(&name).state, TaskState::SetupFailed);
        assert!(f.kinds(&name).contains(&EventKind::CredentialFailed));
        assert_never_called(&f.w.all_calls(), "crabbox", &["warmup"]);
        f.repo_cfg(json!({ "agent": { "location": "host" } }));
        let host = f.name("host");
        assert_eq!(f.start(&host).state, TaskState::Working);
    }
);

parity!(
    namespace_without_enforce_egress_warns_at_start,
    covers = ["A-21"],
    tags = [],
    {
        let f = fx_preset(crabbox::Preset::NamespaceDevbox, UserConfig::default());
        let o = f.try_start(&f.name("ns"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "no warning: {}", o.stdout);
        let local = fx();
        let quiet = local.try_start(&local.name("lc"), &StartOpts::default());
        assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
    }
);

parity!(
    driver_chatter_never_corrupts_json_or_leaks_vocabulary,
    covers = ["A-22", "A-103"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["warmup"])
                .stdout("lease cbx_1 slug={flag:--slug} pond\nconsole.example\n")
                .stderr("SSH: lease slug pond\n")],
        );
        let name = f.name("json");
        let o = f.try_start(&name, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let mut outs = vec![o.stdout.clone()];
        let listed = f.m.list(&f.w, &f.repo.path);
        outs.push(listed.stdout.clone());
        outs.push(f.try_close(&name).stdout);
        for out in &outs {
            assert_pure_json(out);
            let lower = out.to_lowercase();
            for word in ["lease", "slug", "pond", "console.example"] {
                assert!(!lower.contains(word), "{word} in {out}");
            }
        }
    }
);

parity!(
    start_returns_after_box_setup_and_agent_launch_in_order,
    covers = ["A-24", "A-30"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["echo setup-marker"], "provision": ["secret.txt"] }));
        std::fs::write(f.repo.path.join("secret.txt"), "s3").unwrap();
        let name = f.name("order");
        let t = f.goal(&name);
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(
            std::fs::read_to_string(Path::new(t.worktree.as_deref().unwrap()).join("secret.txt"))
                .unwrap(),
            "s3"
        );
        let calls = f.w.all_calls();
        let warm = call_seq(&calls, |c| is_verb(c, "crabbox", "warmup"));
        let setup = call_seq(&calls, |c| {
            is_verb(c, "crabbox", "run") && c.argv.iter().any(|a| a.contains("setup-marker"))
        });
        let agent = call_seq(&calls, is_bg);
        assert!(warm < setup && setup < agent, "{warm} {setup} {agent}");
    }
);

parity!(
    box_size_selects_class_and_size_env,
    covers = ["A-26"],
    tags = [],
    {
        for (size, class, env) in [
            ("small", "standard", "S"),
            ("medium", "fast", "M"),
            ("large", "large", "L"),
            ("xlarge", "beast", "XL"),
        ] {
            let f = fx();
            f.repo_cfg(json!({ "box": { "size": size } }));
            let name = f.name("sz");
            let t = f.start(&name);
            let warm = f.verb("crabbox", "warmup").remove(0);
            let slug = warm.flag("--slug").expect("slug");
            assert!(slug.starts_with(&format!("moor-{name}-")), "{slug}");
            assert_eq!(warm.flag("--class").as_deref(), Some(class));
            assert_eq!(
                warm.env.get("CRABBOX_NAMESPACE_SIZE").map(String::as_str),
                Some(env)
            );
            assert_eq!(warm.flag("--provider").as_deref(), Some("local-container"));
            assert_eq!(
                t.boxed.unwrap().handle,
                Some(format!("local-container/{slug}"))
            );
            assert_eq!(
                std::fs::canonicalize(&warm.cwd).ok(),
                std::fs::canonicalize(t.worktree.unwrap()).ok()
            );
        }
    }
);

parity!(
    egress_policy_reaches_only_sprites_and_enforcing_namespace,
    covers = ["A-27"],
    tags = [],
    {
        let egress = json!({ "egress": ["example.com"] });
        let sprites = fx_preset(crabbox::Preset::Sprites, UserConfig::default());
        sprites.repo_cfg(egress.clone());
        sprites.start(&sprites.name("sp"));
        let warm = sprites.verb("crabbox", "warmup").remove(0);
        let allow = warm.flag("--sprites-network-allow").expect("allow list");
        assert!(allow.split(',').any(|d| d == "example.com"), "{allow}");

        let local = fx();
        local.repo_cfg(egress.clone());
        local.start(&local.name("lc"));
        let warm = local.verb("crabbox", "warmup").remove(0);
        assert_eq!(warm.flag("--sprites-network-allow"), None);
        assert!(!warm.env.contains_key("CRABBOX_NAMESPACE_EGRESS_DOMAINS"));

        let ns = fx_preset(crabbox::Preset::NamespaceDevbox, UserConfig::default());
        ns.repo_cfg(egress);
        let cfg = json!({
            "box": { "provider": "namespace-devbox", "enforce_egress": true }
        });
        write_json(&ns.w.config_path(), cfg);
        ns.start(&ns.name("ns"));
        let warm = ns.verb("crabbox", "warmup").remove(0);
        let domains = warm
            .env
            .get("CRABBOX_NAMESPACE_EGRESS_DOMAINS")
            .expect("domains");
        assert!(domains.split(',').any(|d| d == "example.com"), "{domains}");
    }
);

fn failed_acquire(f: &Fx, name: &str) -> TaskView {
    crabbox::fail_quota(&f.w);
    let o = f.try_start(name, &StartOpts::default());
    assert!(o.class.is_err());
    let row = f.row(name);
    assert_eq!(row.state, TaskState::SetupFailed);
    assert!(f.kinds(name).contains(&EventKind::BoxFailed));
    row
}

parity!(
    acquire_failure_releases_attempted_handle_and_marks_box_failed,
    covers = ["A-28"],
    tags = [],
    {
        let f = fx();
        let name = f.name("fail");
        let row = failed_acquire(&f, &name);
        assert_eq!(box_state(&row), BoxState::Failed);
        let slug = crabbox::slug(&f.w).expect("slug");
        let stop = f.verb("crabbox", "stop").remove(0);
        assert_eq!(stop.flag("--id"), Some(slug));
        assert_eq!(
            stop.env
                .get("CRABBOX_NAMESPACE_DELETE_ON_RELEASE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(f.n("crabbox", "stop"), 1);
    }
);

parity!(
    acquire_failure_with_not_found_release_marks_box_gone,
    covers = ["A-28"],
    tags = [],
    {
        let f = fx();
        crabbox::not_found(&f.w);
        let row = failed_acquire(&f, &f.name("nf"));
        assert_eq!(box_state(&row), BoxState::Gone);
        assert!(row.boxed.unwrap().handle.is_none());
    }
);

parity!(
    acquire_failure_with_release_error_keeps_original_failure,
    covers = ["A-28"],
    tags = [],
    {
        let f = fx();
        crabbox::fail(&f.w, "stop");
        let row = failed_acquire(&f, &f.name("re"));
        assert_eq!(box_state(&row), BoxState::Failed);
        assert!(row.boxed.unwrap().handle.is_some());
    }
);

parity!(
    restart_after_acquire_failure_gets_a_new_box,
    covers = ["A-29"],
    tags = [],
    {
        let f = fx();
        let name = f.name("retry");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["warmup"])
                .exit(2)
                .stderr("quota exceeded for plan\n")
                .times(1)],
        );
        assert!(f.try_start(&name, &StartOpts::default()).class.is_err());
        assert_eq!(f.row(&name).state, TaskState::SetupFailed);
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert_eq!(box_state(&again), BoxState::Ready);
        assert_eq!(f.n("crabbox", "warmup"), 2);
        assert_eq!(f.rows(&name).len(), 1);
    }
);

parity!(
    failing_setup_keeps_task_working_with_box_and_agent,
    covers = ["A-03", "A-31", "A-34"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("setup");
        let o = f.try_start(
            &name,
            &StartOpts {
                goal: Some("go".into()),
                ..Default::default()
            },
        );
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty());
        let t = o.ok();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(box_state(t), BoxState::Ready);
        assert!(f.kinds(&name).contains(&EventKind::SetupFailed));
        assert_eq!(f.n("crabbox", "stop"), 0);
        assert_eq!(f.bg_calls().len(), 1);
        let run = f
            .verb("crabbox", "run")
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a.contains("failing-step")))
            .expect("setup run");
        assert!(run.argv.iter().any(|a| a == "--no-hydrate"));
        assert!(!run.argv.iter().any(|a| a == "--no-sync"));
        assert_eq!(
            run.env
                .get("CRABBOX_SYNC_GIT_SEED_SOURCE")
                .map(String::as_str),
            Some("local")
        );
    }
);

parity!(
    rerun_after_setup_fix_keeps_box_and_clears_warnings,
    covers = ["A-31"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("fix");
        f.start(&name);
        f.repo_cfg(json!({ "setup": ["fixed-step"] }));
        let o = f.try_start(&name, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(f.n("crabbox", "warmup"), 1);
        assert_eq!(f.n("crabbox", "stop"), 0);
    }
);

parity!(
    start_after_setup_failure_reruns_setup_only_once,
    covers = ["A-31", "B-54"],
    tags = ["new_only(SetupRetry)"],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("retry");
        f.start(&name);
        f.repo_cfg(json!({ "setup": ["fixed-step"] }));
        let fixed_runs = |f: &Fx| {
            f.verb("crabbox", "run")
                .into_iter()
                .filter(|c| c.argv.iter().any(|a| a.contains("fixed-step")))
                .count()
        };
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert_eq!(fixed_runs(&f), 1);
        assert_eq!(f.n("crabbox", "warmup"), 1);
        f.start(&name);
        assert_eq!(fixed_runs(&f), 1);
    }
);

parity!(
    provision_copies_and_symlinks_and_missing_source_does_not_stop_start,
    covers = ["A-32"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(
            json!({ "provision": ["secret.txt", { "path": "linked.txt", "mode": "symlink" }] }),
        );
        std::fs::write(f.repo.path.join("secret.txt"), "s3").unwrap();
        std::fs::write(f.repo.path.join("linked.txt"), "l").unwrap();
        let t = f.start(&f.name("prov"));
        let wt = std::path::PathBuf::from(t.worktree.unwrap());
        assert_eq!(
            std::fs::read_to_string(wt.join("secret.txt")).unwrap(),
            "s3"
        );
        assert!(!wt.join("secret.txt").is_symlink());
        assert_eq!(
            std::fs::read_link(wt.join("linked.txt")).unwrap(),
            f.repo.path.join("linked.txt")
        );

        f.repo_cfg(json!({ "provision": ["absent.txt"] }));
        let o = f.try_start(&f.name("miss"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(!o.warnings.is_empty());
    }
);

fn dotfiles_fx(location: &str) -> (Fx, std::path::PathBuf) {
    let f = fx();
    let dir = f.w.root().join("dots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".bashrc"), "# dots\n").unwrap();
    std::fs::write(
        dir.join("install.sh"),
        "#!/bin/sh\ntouch \"$HOME/dotfiles-ran\"\n",
    )
    .unwrap();
    write_json(
        &f.w.config_path(),
        json!({ "dotfiles": { "dir": dir.to_str().unwrap() } }),
    );
    f.repo_cfg(json!({ "setup": ["echo setup-marker"], "agent": { "location": location } }));
    crabbox::ssh_wrapper(&f.w);
    parity::fake::ssh::install(&f.w);
    let home = f.w.box_root().join("home");
    (f, home)
}

fn run_with(f: &Fx, needle: &str) -> Vec<Call> {
    f.verb("crabbox", "run")
        .into_iter()
        .filter(|c| c.argv.iter().any(|a| a.contains(needle)))
        .collect()
}

parity!(
    dotfiles_dir_uploaded_and_installed_after_setup,
    covers = ["A-33"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("box");
        let t = f.start(&f.name("dots"));
        assert_eq!(t.state, TaskState::Working);
        assert!(home.join(".moor-dotfiles/.bashrc").exists());
        assert!(home.join("dotfiles-ran").exists());
        let setup = run_with(&f, "setup-marker")[0].seq;
        let dots = run_with(&f, "MOOR_DOTFILES=1")[0].seq;
        assert!(setup < dots);
    }
);

parity!(
    dotfiles_failure_sets_setup_failed_keeps_box_and_retry_applies,
    covers = ["A-33"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("box");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["run"])
                .contains("MOOR_DOTFILES=1")
                .exit(9)
                .stderr("install failed\n")
                .times(1)],
        );
        let name = f.name("dotfail");
        assert!(f.try_start(&name, &StartOpts::default()).class.is_err());
        let row = f.row(&name);
        assert_eq!(row.state, TaskState::SetupFailed);
        assert_eq!(box_state(&row), BoxState::Ready);
        assert_eq!(f.n("crabbox", "stop"), 0);
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert!(home.join("dotfiles-ran").exists());
        assert_eq!(f.n("crabbox", "warmup"), 1);
    }
);

parity!(
    dotfiles_skipped_for_host_agent_tasks,
    covers = ["A-33"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("host");
        assert_eq!(f.start(&f.name("hostdots")).state, TaskState::Working);
        assert!(run_with(&f, "setup-marker").len() == 1);
        assert!(run_with(&f, "MOOR_DOTFILES=1").is_empty());
        assert!(!home.join(".moor-dotfiles").exists());
        assert_never_called(&f.w.all_calls(), "ssh", &[]);
    }
);

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
    covers = ["A-49"],
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
    covers = ["A-92"],
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
    close_aborts_when_agent_cannot_be_stopped_and_is_still_active,
    covers = ["A-91"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules("claude", [Rule::on(&["stop"]).exit(1).stderr("busy\n")]);
        let name = f.name("stuck");
        f.goal(&name);
        assert!(f.try_close(&name).class.is_err());
        assert_ne!(f.row(&name).state, TaskState::Closed);
        assert_eq!(f.n("crabbox", "stop"), 0);
        assert!(f.m.sessions(&f.w, &name)[0].live);
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
    covers = ["A-93", "A-95"],
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

fn commit_in(f: &Fx, wt: &Path, file: &str) -> String {
    std::fs::write(wt.join(file), "w").unwrap();
    git_in(f, wt, &["add", file]);
    git_in(f, wt, &["commit", "-q", "-m", "work"]);
    git_in(f, wt, &["rev-parse", "HEAD"])
}

fn closed_task(f: &Fx, base: &str) -> (String, std::path::PathBuf) {
    let name = f.name(base);
    let t = f.start(&name);
    f.close(&name);
    (name, std::path::PathBuf::from(t.worktree.unwrap()))
}

fn branch_exists(f: &Fx, name: &str) -> bool {
    !git_in(
        f,
        &f.repo.path,
        &["branch", "--list", &format!("moor/{name}")],
    )
    .is_empty()
}

fn remove(
    f: &Fx,
    name: &str,
    discard: bool,
) -> parity::adapter::Outcome<parity::adapter::RemoveView> {
    f.m.remove(&f.w, &f.repo.path, name, discard)
}

parity!(
    remove_refuses_open_unknown_and_bound_tasks,
    covers = ["A-98"],
    tags = [],
    {
        let f = fx();
        let open = f.name("open");
        let t = f.start(&open);
        assert!(remove(&f, &open, true).class.is_err());
        assert!(Path::new(t.worktree.as_deref().unwrap()).is_dir());
        assert!(remove(&f, &f.name("nobody"), false).class.is_err());

        let wt = bound_worktree(&f, "feature/z");
        let bound = f.name("bound");
        f.start_with(&bound, &bind_opts(&wt));
        f.close(&bound);
        assert!(remove(&f, &bound, true).class.is_err());
        assert!(wt.is_dir());
        let listing = git_in(&f, &f.repo.path, &["branch", "--list", "feature/z"]);
        assert!(listing.contains("feature/z"));
    }
);

parity!(
    remove_refuses_unpushed_commits_until_discard,
    covers = ["A-99"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "unp");
        commit_in(&f, &wt, "work.txt");
        assert!(remove(&f, &name, false).class.is_err());
        assert!(wt.is_dir() && branch_exists(&f, &name));
        let done = remove(&f, &name, true);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let view = done.ok();
        assert!(view.worktree_removed && view.branch_deleted && view.discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
    }
);

parity!(
    remove_refuses_uncommitted_files_but_ignores_moor_dirs,
    covers = ["A-99"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "dirty");
        std::fs::create_dir_all(wt.join(".moor")).unwrap();
        std::fs::write(wt.join(".moor/setup.log"), "x").unwrap();
        std::fs::write(wt.join("scratch.txt"), "x").unwrap();
        assert!(remove(&f, &name, false).class.is_err());
        assert!(wt.is_dir());
        std::fs::remove_file(wt.join("scratch.txt")).unwrap();
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        assert!(!done.ok().discarded);
        assert!(!wt.exists());
    }
);

parity!(
    remove_allows_commits_reachable_from_a_remote_ref,
    covers = ["A-99", "A-100"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "pushed");
        commit_in(&f, &wt, "work.txt");
        git_in(&f, &wt, &["push", "-q", "origin", &format!("moor/{name}")]);
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        assert!(!done.ok().discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
    }
);

parity!(
    remove_deletes_checkout_and_branch_and_repeat_is_a_noop,
    covers = ["A-100", "A-101"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "clean");
        let before = f.m.events(&f.w, &name);
        let stops = f.n("crabbox", "stop");
        let calls_before = f.w.calls("crabbox").len();
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let view = done.ok();
        assert!(view.worktree_removed && view.branch_deleted && !view.discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
        assert_eq!(f.row(&name).state, TaskState::Closed);
        assert_eq!(f.n("crabbox", "stop"), stops);
        assert_eq!(f.w.calls("crabbox").len(), calls_before);
        assert!(f.m.events(&f.w, &name).len() > before.len());
        assert!(f.w.state_dir().join("registry.db").is_file());

        let again = remove(&f, &name, false);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert!(!again.ok().worktree_removed && !again.ok().branch_deleted);
    }
);

parity!(
    remove_uses_herdr_when_it_still_lists_the_worktree,
    covers = ["A-100"],
    tags = [],
    {
        let f = fx();
        let name = f.name("viaherdr");
        let t = f.start(&name);
        let wt = t.worktree.unwrap();
        f.close(&name);
        let branch = format!("moor/{name}");
        herdr::worktrees(&f.w, &[(&branch, &wt, Some("ws-open"))]);
        f.w.prepend_rules(
            "herdr",
            [Rule::on(&["worktree", "remove"]).exec(format!(
                "git -C '{}' worktree remove --force '{wt}' && printf '%s' '{{\"result\":{{}}}}'",
                f.repo.path.display()
            ))],
        );
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let calls = herdr::calls_of(&f.w, "worktree", "remove");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].flag("--workspace").as_deref(), Some("ws-open"));
        assert!(calls[0].argv.iter().any(|a| a == "--force"));
        assert!(!Path::new(&wt).exists() && !branch_exists(&f, &name));
    }
);

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

parity!(
    credential_values_stay_out_of_outputs_registry_logs_and_driver_argv,
    covers = ["A-102"],
    tags = [],
    {
        let f = fx();
        let (host, mcp) = ("FAKE-SECRET-4242", "s3cr3t-MCP-77");
        write_json(
            &f.w.config_path(),
            json!({
                "credentials": {
                    "TOKEN": { "cmd": "printf FAKE-SECRET-%s 4242" },
                    "PROBE_TOKEN": { "cmd": format!("echo {mcp}") }
                },
                "mcp_servers": { "probe": {
                    "command": "python3", "args": ["server.py"],
                    "env": { "PROBE_TOKEN": "${PROBE_TOKEN}" }
                } }
            }),
        );
        f.repo_cfg(json!({ "credentials": ["TOKEN"], "setup": ["echo ok"] }));
        let name = f.name("leak");
        let started = f.try_start(
            &name,
            &StartOpts {
                goal: Some("go".into()),
                ..Default::default()
            },
        );
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let mcp_file = f.w.state_dir().join("mcp").join(format!("{name}.json"));
        let body = std::fs::read_to_string(&mcp_file).expect("mcp file written");
        assert!(body.contains(mcp));
        let listed = f.m.list(&f.w, &f.repo.path);
        let mut outputs = vec![started.stdout, started.stderr, listed.stdout, listed.stderr];
        for secret in [host, mcp] {
            for o in &outputs {
                assert!(!o.contains(secret), "{secret} in output");
            }
            for p in files_under(&f.w.state_dir())
                .into_iter()
                .filter(|p| *p != mcp_file)
            {
                assert!(
                    !contains(&std::fs::read(&p).unwrap(), secret),
                    "{}",
                    p.display()
                );
            }
            for c in f.w.all_calls() {
                let text = format!("{} {} {:?}", c.tool, c.argv.join(" "), c.env);
                assert!(!text.contains(secret), "{secret} in {text}");
            }
        }
        let closed = f.try_close(&name);
        outputs.push(closed.stdout);
        outputs.push(closed.stderr);
        assert!(outputs
            .iter()
            .all(|o| !o.contains(host) && !o.contains(mcp)));
        assert!(!mcp_file.exists(), "mcp file survives close");
    }
);

parity!(
    one_live_task_per_name_and_events_are_append_only,
    covers = ["A-104"],
    tags = [],
    {
        let f = fx();
        let name = f.name("uniq");
        let first = f.start(&name);
        let snapshot = f.m.events(&f.w, &name);
        let again = f.start(&name);
        assert_eq!(first.id, again.id);
        assert_eq!(f.rows(&name).len(), 1);
        f.list();
        f.close(&name);
        let after = f.m.events(&f.w, &name);
        assert!(after.len() > snapshot.len());
        assert_eq!(after[..snapshot.len()], snapshot[..]);
    }
);

parity!(
    simultaneous_starts_of_one_name_yield_one_task_and_one_box,
    covers = ["A-105"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules("crabbox", [Rule::on(&["warmup"]).sleep_ms(1500)]);
        let name = f.name("race");
        let opts = StartOpts::default();
        let a = f.m.start_detached(&f.w, &f.repo.path, &name, &opts);
        let b = f.m.start_detached(&f.w, &f.repo.path, &name, &opts);
        let outs = [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()];
        assert!(outs.iter().any(|o| o.status.success()), "no start won");
        assert_eq!(f.rows(&name).len(), 1);
        assert_eq!(f.n("crabbox", "warmup"), 1);
    }
);

parity!(
    bound_worktree_config_drives_setup,
    covers = [],
    tags = ["new_only(WorktreeConfig)"],
    {
        let f = fx();
        let wt = bound_worktree(&f, "feature/cfg");
        write_json(
            &wt.join(".moor.yaml"),
            json!({ "setup": ["echo wt-config-marker"] }),
        );
        f.start_with(&f.name("cfg"), &bind_opts(&wt));
        assert_eq!(run_with(&f, "wt-config-marker").len(), 1);
    }
);
