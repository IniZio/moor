use std::path::{Path, PathBuf};

use parity::adapter::{BoxState, EventKind, ExitClass, RunOpts, StartOpts, TaskState};
use parity::assert::{assert_never_called, assert_order};
use parity::fake::{crabbox, herdr, Call};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

struct Fx {
    w: World,
    repo: Repo,
    name: String,
    wt: PathBuf,
    slug: String,
}

fn write_user(w: &World, crabbox_keys: Value) {
    let doc = json!({ "drivers": { "crabbox": crabbox_keys } });
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

fn commit_all(w: &World, dir: &Path, msg: &str) {
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", msg]] {
        let o = w
            .command("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
}

fn git_out(w: &World, dir: &Path, args: &[&str]) -> String {
    let o = w
        .command("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn prepare(repo_cfg: Option<Value>, crabbox_keys: Value) -> (World, Repo) {
    let w = world();
    let repo = Repo::create(&w, "widget");
    write_user(&w, crabbox_keys);
    crabbox::install(&w);
    crabbox::cp(&w);
    herdr::install(&w);
    if let Some(doc) = repo_cfg {
        std::fs::write(repo.path.join(".moor.yaml"), doc.to_string()).expect("write repo config");
    }
    (w, repo)
}

fn start_task(w: &World, repo: &Repo, base: &str) -> (String, PathBuf, String) {
    let name = w.task_name(base);
    let o = moor().start(w, &repo.path, &name, &StartOpts::default());
    assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    let wt = PathBuf::from(o.ok().worktree.clone().expect("worktree"));
    let slug = crabbox::slug(w).expect("warmup slug");
    (name, wt, slug)
}

fn fx_with(repo_cfg: Option<Value>) -> Fx {
    let (w, repo) = prepare(repo_cfg, json!({}));
    let (name, wt, slug) = start_task(&w, &repo, "rn");
    Fx {
        w,
        repo,
        name,
        wt,
        slug,
    }
}

fn fx() -> Fx {
    fx_with(None)
}

fn verb_calls(w: &World, verb: &str) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
        .collect()
}

fn user_runs(w: &World) -> Vec<Call> {
    verb_calls(w, "run")
        .into_iter()
        .filter(|c| !c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
        .collect()
}

fn has(c: &Call, flag: &str) -> bool {
    c.argv.iter().any(|a| a == flag)
}

fn go(f: &Fx, argv: &[&str]) -> parity::adapter::Outcome<parity::adapter::RunResult> {
    moor().run(&f.w, &f.repo.path, &f.name, argv, &RunOpts::default())
}

fn ran_events(w: &World, name: &str) -> usize {
    moor()
        .events(w, name)
        .iter()
        .filter(|e| e.kind == EventKind::Ran)
        .count()
}

parity!(
    run_exit_code_and_output_propagate,
    covers = ["B-01", "B-05", "B-11"],
    tags = [],
    {
        let f = fx();
        let o = go(&f, &["sh", "-c", "echo parity-out; exit 7"]);
        assert_eq!(o.exit, 7, "remote exit code reaches the caller");
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stdout.contains("parity-out"), "{}", o.stdout);
        let r = o.value.as_ref().expect("run result");
        assert_eq!((r.exit, r.interrupted), (7, false));
        let ok = go(&f, &["true"]);
        assert_eq!(ok.class, ExitClass::Ok, "{}", ok.stderr);
        assert_eq!(ran_events(&f.w, &f.name), 2);
        let runs = user_runs(&f.w);
        assert_eq!(runs.len(), 2);
        let c = &runs[0];
        assert_eq!(c.flag("--provider").as_deref(), Some("local-container"));
        assert_eq!(c.flag("--id").as_deref(), Some(f.slug.as_str()));
        assert!(has(c, "--no-hydrate") && !has(c, "--no-sync"));
        assert_eq!(
            c.env
                .get("CRABBOX_SYNC_GIT_SEED_SOURCE")
                .map(String::as_str),
            Some("local")
        );
        assert_eq!(
            std::fs::canonicalize(&c.cwd).ok(),
            std::fs::canonicalize(&f.wt).ok()
        );
    }
);

parity!(
    run_command_quoting_and_shell,
    covers = ["B-02", "B-03", "B-18"],
    tags = [],
    {
        let f = fx();
        let multi = go(&f, &["printf", "%s|%s", "a b", "it's"]);
        assert_eq!(multi.class, ExitClass::Ok, "{}", multi.stderr);
        assert_eq!(multi.stdout, "a b|it's");
        let one = go(&f, &["echo one; echo two | tr a-z A-Z"]);
        assert!(one.stdout.contains("one") && one.stdout.contains("TWO"));
        let pf = go(&f, &["set -o pipefail; false | true; echo rc=$?"]);
        assert!(pf.stdout.contains("rc=1"), "{} {}", pf.stdout, pf.stderr);
        let plain = go(&f, &["false | true; echo rc=$?"]);
        assert_eq!(plain.class, ExitClass::Ok);
        assert!(plain.stdout.contains("rc=0"));
    }
);

parity!(
    run_sync_flag_follows_no_sync,
    covers = ["B-04"],
    tags = [],
    {
        let f = fx();
        go(&f, &["true"]);
        let quick = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["true"],
            &RunOpts {
                no_sync: true,
                ..Default::default()
            },
        );
        assert_eq!(quick.class, ExitClass::Ok, "{}", quick.stderr);
        let runs = user_runs(&f.w);
        assert!(!has(&runs[0], "--no-sync"));
        assert!(has(&runs[1], "--no-sync"));
        assert!(runs.iter().all(|c| has(c, "--no-hydrate")));
        assert_eq!(ran_events(&f.w, &f.name), 2);
    }
);

parity!(run_size_maps_to_class, covers = ["B-07"], tags = [], {
    let cases = [
        (None, "standard"),
        (Some("medium"), "fast"),
        (Some("large"), "large"),
        (Some("xlarge"), "beast"),
    ];
    for (size, class) in cases {
        let cfg = size.map(|s| json!({ "box": { "size": s } }));
        let f = fx_with(cfg);
        let o = go(&f, &["true"]);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let run = user_runs(&f.w).remove(0);
        assert_eq!(run.flag("--class").as_deref(), Some(class), "size {size:?}");
        let warm = verb_calls(&f.w, "warmup").remove(0);
        assert_eq!(warm.flag("--class").as_deref(), Some(class));
    }
});

parity!(
    run_applies_repo_run_env_and_path,
    covers = ["B-08"],
    tags = [],
    {
        let cfg =
            json!({ "run": { "env": { "PARITY_FOO": "bar baz" }, "path": ["/opt/parity-x"] } });
        let f = fx_with(Some(cfg));
        let o = go(&f, &["printf '%s|%s' \"$PARITY_FOO\" \"$PATH\""]);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            o.stdout.starts_with("bar baz|/opt/parity-x:"),
            "{}",
            o.stdout
        );
    }
);

parity!(
    run_refused_without_a_ready_box,
    covers = ["B-09", "B-10"],
    tags = [],
    {
        let f = fx();
        let m = moor();
        let before = user_runs(&f.w).len();
        let none = m.run(&f.w, &f.repo.path, "nope", &["true"], &RunOpts::default());
        assert!(none.is_err());
        let empty = m.run(&f.w, &f.repo.path, &f.name, &[], &RunOpts::default());
        assert!(empty.is_err());
        assert_eq!(user_runs(&f.w).len(), before);

        crabbox::gone(&f.w, &f.slug);
        let listed = m.list(&f.w, &f.repo.path);
        let row = listed.ok().iter().find(|t| t.name == f.name).cloned();
        assert_eq!(row.unwrap().boxed.unwrap().state, BoxState::Gone);
        let gone = go(&f, &["true"]);
        assert!(gone.is_err());
        assert!(gone.value.is_none());
        assert_eq!(user_runs(&f.w).len(), before);
        let kinds: Vec<_> = m
            .events(&f.w, &f.name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(
            kinds.contains(&EventKind::Ran),
            "waiters must see a ran event"
        );
    }
);

parity!(
    run_refused_while_waiting_for_a_box,
    covers = ["B-10"],
    tags = [],
    {
        let (w, repo) = prepare(None, json!({ "max_boxes": 1 }));
        let (_first, _wt, _slug) = start_task(&w, &repo, "cap");
        let second = w.task_name("wait");
        let o = moor().start(&w, &repo.path, &second, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::WaitingForBox);
        let r = moor().run(&w, &repo.path, &second, &["true"], &RunOpts::default());
        assert!(r.is_err());
        assert!(user_runs(&w).is_empty());
    }
);

parity!(
    run_quiet_keeps_program_output_and_exit,
    covers = [],
    tags = [],
    {
        let f = fx();
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sh", "-c", "echo quiet-out; exit 3"],
            &RunOpts {
                quiet: true,
                ..Default::default()
            },
        );
        assert_eq!(o.exit, 3);
        assert!(o.stdout.contains("quiet-out"), "{}", o.stdout);
    }
);

parity!(
    run_interrupt_marks_run_interrupted,
    covers = ["B-12"],
    tags = [],
    {
        let f = fx();
        crabbox::block(&f.w, "run");
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sleep", "600"],
            &RunOpts {
                interrupt: true,
                ..Default::default()
            },
        );
        assert!(o.is_err(), "interrupted run is not a success");
        assert_eq!(o.exit, 130);
        let r = o.value.as_ref().expect("a ran event records the interrupt");
        assert!(r.interrupted);
        assert_eq!(ran_events(&f.w, &f.name), 1);
        assert_order(&f.w.all_calls(), &[("crabbox", &["run"])]);
    }
);

parity!(
    run_child_killed_by_signal_is_interrupted,
    covers = ["B-12"],
    tags = [],
    {
        let f = fx();
        crabbox::die_on_run(&f.w);
        let o = go(&f, &["true"]);
        assert!(o.is_err());
        assert_eq!(o.exit, 130);
        assert!(o.value.as_ref().expect("ran event").interrupted);
    }
);

fn copied_events(w: &World, name: &str) -> usize {
    moor()
        .events(w, name)
        .iter()
        .filter(|e| e.kind == EventKind::Copied)
        .count()
}

fn seed_box_file(f: &Fx, rel: &str, body: &str) -> PathBuf {
    let p = f.w.box_root().join("work").join(rel);
    std::fs::write(&p, body).unwrap();
    p
}

fn host_dir(f: &Fx, name: &str) -> PathBuf {
    let d = f.w.tmp_dir().join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

parity!(
    cp_absolute_path_copies_without_sync,
    covers = ["B-20", "B-23"],
    tags = [],
    {
        let f = fx();
        let src = seed_box_file(&f, "out.txt", "payload-1");
        let dest = host_dir(&f, "cp-abs").join("got.txt");
        let runs_before = user_runs(&f.w).len();
        let o = moor().cp(&f.w, &f.repo.path, &f.name, &src.to_string_lossy(), &dest);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "payload-1");
        let cps = verb_calls(&f.w, "cp");
        assert_eq!(cps.len(), 1);
        let c = &cps[0];
        assert_eq!(c.flag("--provider").as_deref(), Some("local-container"));
        assert_eq!(c.flag("--id").as_deref(), Some(f.slug.as_str()));
        assert!(c.argv.iter().any(|a| a.ends_with(&*src.to_string_lossy())));
        assert_eq!(c.argv.last().map(String::as_str), dest.to_str());
        assert_eq!(
            std::fs::canonicalize(&c.cwd).ok(),
            std::fs::canonicalize(&f.wt).ok()
        );
        assert_eq!(
            user_runs(&f.w).len(),
            runs_before,
            "cp never runs a command"
        );
        assert_eq!(copied_events(&f.w, &f.name), 1);
    }
);

parity!(
    cp_nonzero_exit_propagates_and_is_recorded,
    covers = ["B-20", "B-23"],
    tags = [],
    {
        let f = fx();
        crabbox::fail(&f.w, "cp");
        let dest = host_dir(&f, "cp-fail").join("got.txt");
        let o = moor().cp(&f.w, &f.repo.path, &f.name, "/nowhere", &dest);
        assert!(o.is_err());
        assert!(!dest.exists());
        assert_eq!(copied_events(&f.w, &f.name), 1);
    }
);

parity!(
    cp_relative_paths_resolve_against_workdir_and_cwd,
    covers = ["B-21", "B-22"],
    tags = [],
    {
        let f = fx();
        seed_box_file(&f, "rel.txt", "payload-2");
        let cwd = host_dir(&f, "cp-rel");
        let o = moor().cp(&f.w, &cwd, &f.name, "./rel.txt", Path::new("out.txt"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(
            std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
            "payload-2"
        );
        let probes: Vec<_> = verb_calls(&f.w, "run");
        let probe = probes
            .iter()
            .find(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
            .expect("workdir probe");
        assert!(has(probe, "--no-sync"));
        let cp = verb_calls(&f.w, "cp").remove(0);
        let want_src = f.w.box_root().join("work/rel.txt");
        assert!(cp
            .argv
            .iter()
            .any(|a| a.ends_with(&*want_src.to_string_lossy())));
        let host = PathBuf::from(cp.argv.last().unwrap());
        assert!(host.is_absolute());
        assert_eq!(
            std::fs::canonicalize(host.parent().unwrap()).unwrap(),
            std::fs::canonicalize(&cwd).unwrap()
        );
    }
);

parity!(
    cp_fails_when_workdir_cannot_be_resolved,
    covers = ["B-21"],
    tags = [],
    {
        let f = fx();
        seed_box_file(&f, "rel.txt", "x");
        crabbox::fail_match(&f.w, "run", "MOOR_WORKDIR", 1);
        let cwd = host_dir(&f, "cp-nowd");
        let o = moor().cp(&f.w, &cwd, &f.name, "rel.txt", Path::new("out.txt"));
        assert!(o.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
    }
);

parity!(
    cp_refused_without_a_ready_box,
    covers = ["B-26"],
    tags = [],
    {
        let f = fx();
        let m = moor();
        let dest = host_dir(&f, "cp-ref").join("o");
        let no_task = m.cp(&f.w, &f.repo.path, "nope", "/x", &dest);
        assert!(no_task.is_err());
        let blank = m.cp(&f.w, &f.repo.path, &f.name, "  ", &dest);
        assert!(blank.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
        crabbox::gone(&f.w, &f.slug);
        m.list(&f.w, &f.repo.path).ok();
        let gone = m.cp(&f.w, &f.repo.path, &f.name, "/x", &dest);
        assert!(gone.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
        assert_never_called(&f.w.all_calls(), "crabbox", &["cp"]);

        let (w, repo) = prepare(None, json!({ "max_boxes": 1 }));
        start_task(&w, &repo, "cap");
        let second = w.task_name("wait");
        let o = moor().start(&w, &repo.path, &second, &StartOpts::default());
        assert_eq!(o.ok().state, TaskState::WaitingForBox);
        let r = moor().cp(&w, &repo.path, &second, "/x", &w.tmp_dir().join("o"));
        assert!(r.is_err());
        assert!(verb_calls(&w, "cp").is_empty());
    }
);

fn put(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn wt_commit(f: &Fx, files: &[(&str, &str)]) -> String {
    for (rel, body) in files {
        put(&f.wt, rel, body);
    }
    commit_all(&f.w, &f.wt, "parity change");
    git_out(&f.w, &f.wt, &["rev-parse", "HEAD"])
}

fn rec(f: &Fx, name: &str, cmd: &str) -> parity::adapter::Outcome<parity::adapter::RunResult> {
    let opts = RunOpts {
        record: Some(name.to_string()),
        ..Default::default()
    };
    moor().run(&f.w, &f.repo.path, &f.name, &[cmd], &opts)
}

fn gate(
    f: &Fx,
    cwd: &Path,
    sha: &str,
    name: &str,
) -> parity::adapter::Outcome<parity::adapter::GateVerdict> {
    moor().gate_check(&f.w, cwd, sha, name, None)
}

parity!(
    record_runs_from_clean_checkout_and_stores_result,
    covers = ["B-31", "B-32", "B-40"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[(".gitignore", "junk.log\n")]);
        put(&f.wt, "junk.log", "stale");
        let cmd = "test ! -e junk.log && echo clean-start";
        let o = rec(&f, "unit", cmd);
        assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
        assert!(o.stdout.contains("clean-start"));
        let r = o.value.as_ref().expect("run result");
        assert_eq!(r.sha.as_deref(), Some(head.as_str()));
        let rows = moor().runs(&f.w, &f.name);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].name.as_str(), rows[0].command.as_str()),
            ("unit", cmd)
        );
        assert_eq!((rows[0].exit, rows[0].interrupted), (0, false));
        assert_eq!(rows[0].sha, head);
        let g = gate(&f, &f.w.tmp_dir(), &head[..7], "unit");
        assert_eq!(g.class, ExitClass::Ok, "{}", g.stderr);
        assert!(g.ok().ok);
    }
);

parity!(
    record_failure_is_stored_and_fails_the_gate,
    covers = ["B-32", "B-36"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[("a.txt", "a")]);
        let o = rec(&f, "unit", "exit 5");
        assert_eq!(o.exit, 5);
        assert_eq!(o.value.as_ref().unwrap().exit, 5);
        let rows = moor().runs(&f.w, &f.name);
        assert_eq!((rows[0].exit, rows[0].interrupted), (5, false));
        let g = gate(&f, &f.w.tmp_dir(), &head, "unit");
        assert_eq!(g.class, ExitClass::Failed);
        let v = g.value.as_ref().expect("verdict json on failure");
        assert!(!v.ok);
        assert_eq!(v.exit, Some(5));
        let none = gate(&f, &f.w.tmp_dir(), &head, "other");
        assert_eq!(none.class, ExitClass::Failed);
        assert_eq!(
            none.value.as_ref().map(|v| (v.ok, v.exit)),
            Some((false, None))
        );
    }
);

parity!(
    record_interrupted_run_fails_the_gate,
    covers = ["B-36"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[("a.txt", "a")]);
        crabbox::block(&f.w, "run");
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sleep 600"],
            &RunOpts {
                record: Some("unit".into()),
                interrupt: true,
                ..Default::default()
            },
        );
        assert!(o.value.as_ref().expect("ran event").interrupted);
        let rows = moor().runs(&f.w, &f.name);
        assert!(rows[0].interrupted);
        let g = gate(&f, &f.w.tmp_dir(), &head, "unit");
        assert_eq!(g.class, ExitClass::Failed);
        assert!(!g.value.as_ref().unwrap().ok);
    }
);

parity!(
    record_refuses_unsafe_trees_before_any_box_call,
    covers = ["B-30"],
    tags = [],
    {
        let f = fx();
        wt_commit(&f, &[(".gitignore", "ignored.txt\n")]);
        let m = moor();
        put(&f.wt, "ignored.txt", "x");
        put(&f.wt, "untracked.txt", "x");
        let dirty = rec(&f, "unit", "true");
        assert!(dirty.is_err(), "untracked file refuses --record");
        std::fs::remove_file(f.wt.join("untracked.txt")).unwrap();
        let no_sync = m.run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["true"],
            &RunOpts {
                record: Some("unit".into()),
                no_sync: true,
                ..Default::default()
            },
        );
        assert!(no_sync.is_err(), "--record needs sync");
        assert!(
            user_runs(&f.w).is_empty(),
            "no Box call for a refused record"
        );
        std::fs::remove_dir_all(&f.wt).unwrap();
        let gone = rec(&f, "unit", "true");
        assert!(gone.is_err(), "missing worktree refuses --record");
        assert!(user_runs(&f.w).is_empty());
        assert!(m.runs(&f.w, &f.name).is_empty());
    }
);

parity!(gate_check_sha_forms, covers = ["B-34"], tags = [], {
    let f = fx();
    let head = wt_commit(&f, &[("a.txt", "a")]);
    assert_eq!(rec(&f, "unit", "true").class, ExitClass::Ok);
    let away = f.w.tmp_dir();
    let short = &head[..7];
    assert!(gate(&f, &away, short, "unit").ok().ok);
    assert!(gate(&f, &away, &short.to_uppercase(), "unit").ok().ok);
    assert!(gate(&f, &away, &head, "unit").ok().ok);
    for bad in ["abc", "zzzz", ""] {
        let o = gate(&f, &away, bad, "unit");
        assert_eq!(o.class, ExitClass::Usage, "sha {bad:?}");
    }
    let unknown_short = gate(&f, &away, "deadb", "unit");
    assert_eq!(unknown_short.class, ExitClass::Usage);
    let unknown_long = gate(&f, &away, "deadbee", "unit");
    assert_eq!(unknown_long.class, ExitClass::Failed);
});

parity!(gate_check_matching_rules, covers = ["B-35"], tags = [], {
    let f = fx();
    let head = wt_commit(&f, &[("a.txt", "a")]);
    assert_eq!(rec(&f, "unit", "echo first").class, ExitClass::Ok);
    assert_eq!(rec(&f, "unit", "exit 4").exit, 4);
    let away = f.w.tmp_dir();
    assert!(
        gate(&f, &away, &head, "unit").ok().ok,
        "an older pass beats a newer fail"
    );
    let m = moor();
    let by_cmd = |c: &str| m.gate_check(&f.w, &away, &head, "unit", Some(c));
    assert!(by_cmd("echo first").ok().ok);
    assert_eq!(by_cmd("exit 4").class, ExitClass::Failed);
    assert_eq!(by_cmd("echo other").class, ExitClass::Failed);
    for tail in ["1111", "2222"] {
        m.seed_run(
            &f.w,
            &parity::adapter::RunRow {
                task: f.name.clone(),
                sha: format!("abcdef0{tail}{}", "0".repeat(29)),
                name: "amb".into(),
                command: "true".into(),
                exit: 0,
                interrupted: false,
            },
        );
    }
    let amb = gate(&f, &away, "abcdef0", "amb");
    assert_eq!(
        amb.class,
        ExitClass::Usage,
        "ambiguous prefix is a usage error"
    );
    assert!(amb.value.is_none_or(|v| !v.ok));
    let exact = gate(&f, &away, &format!("abcdef01111{}", "0".repeat(29)), "amb");
    assert!(exact.ok().ok);
});

parity!(
    repo_config_errors_block_start,
    covers = ["B-38", "B-57"],
    tags = [],
    {
        let docs = [
            json!({ "record": { "keep": ["node_modules"] } }),
            json!({ "setup": [""] }),
            json!({ "teardown": ["  "] }),
            json!({ "tools": [""] }),
            json!({ "provision": ["../outside"] }),
            json!({ "provision": ["/etc/hostname"] }),
        ];
        for doc in docs {
            let (w, repo) = prepare(Some(doc.clone()), json!({}));
            let o = moor().start(&w, &repo.path, &w.task_name("cfg"), &StartOpts::default());
            assert!(o.is_err(), "{doc} must be a config error");
            assert!(verb_calls(&w, "warmup").is_empty(), "{doc}");
        }
    }
);

fn probe(f: &Fx, dirs: &[&str]) -> String {
    let mut cmd = String::new();
    for d in dirs {
        cmd.push_str(&format!(
            "if [ -e '{d}/dep.txt' ]; then echo \"KEPT {d}\"; else echo \"GONE {d}\"; fi; "
        ));
    }
    for d in dirs {
        cmd.push_str(&format!("mkdir -p '{d}'; echo x > '{d}/dep.txt'; "));
    }
    let o = rec(f, "unit", &cmd);
    assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
    o.stdout
}

fn kept(out: &str, d: &str) -> bool {
    assert!(
        out.contains(&format!("KEPT {d}")) || out.contains(&format!("GONE {d}")),
        "{d} not probed in {out}"
    );
    out.contains(&format!("KEPT {d}"))
}

const LOCKS: &[(&str, &str)] = &[
    ("package-lock.json", "node_modules"),
    ("uv.lock", ".venv"),
    ("Gemfile.lock", "vendor/bundle"),
    ("composer.lock", "vendor/composer-deps"),
    ("Podfile.lock", "Pods"),
];

parity!(
    record_keeps_dependency_dirs_by_lockfile,
    covers = ["B-43", "B-44", "B-45"],
    tags = [],
    {
        let f = fx();
        let mut files: Vec<(&str, &str)> = LOCKS.iter().map(|(l, _)| (*l, "v1")).collect();
        files.push((
            ".gitignore",
            "node_modules/\n.venv/\nvendor/\nPods/\n.next/\ndist/\n",
        ));
        wt_commit(&f, &files);
        let dirs: Vec<&str> = LOCKS.iter().map(|(_, d)| *d).collect();
        let first = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&first, d)), "{first}");
        let second = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| kept(&second, d)), "{second}");

        wt_commit(&f, &[("uv.lock", "v2")]);
        let after_edit = probe(&f, &dirs);
        assert!(!kept(&after_edit, ".venv"), "{after_edit}");
        assert!(kept(&after_edit, "node_modules"), "{after_edit}");

        wt_commit(&f, &[(".nvmrc", "20")]);
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
        wt_commit(&f, &[(".nvmrc", "22")]);
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_cache_key_includes_setup_text,
    covers = ["B-42", "B-44"],
    tags = [],
    {
        let f = fx_with(Some(json!({ "setup": ["echo one >> .moor/setup-runs"] })));
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let runs = |f: &Fx| {
            std::fs::read_to_string(f.wt.join(".moor/setup-runs"))
                .unwrap_or_default()
                .lines()
                .count()
        };
        let at_start = runs(&f);
        assert_eq!(at_start, 1, "setup ran once at start");
        probe(&f, &["node_modules"]);
        assert_eq!(runs(&f), 2, "setup is re-run after the clean");
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
        let cfg = json!({ "setup": ["echo one >> .moor/setup-runs", "true"] });
        std::fs::write(f.repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_failing_setup_step_stops_the_command,
    covers = ["B-42"],
    tags = [],
    {
        let cfg = json!({ "setup": ["test ! -e .moor/break-setup"] });
        let f = fx_with(Some(cfg));
        wt_commit(&f, &[("a.txt", "a")]);
        put(&f.wt, ".moor/break-setup", "1");
        let marker = f.w.box_root().join("command-ran");
        let o = rec(&f, "unit", &format!("touch '{}'", marker.display()));
        assert!(o.is_err());
        assert!(!marker.exists(), "command must not run after failed prep");
        let rows = moor().runs(&f.w, &f.name);
        assert_ne!(rows[0].exit, 0);
    }
);

parity!(
    record_tool_caches_follow_the_node_lockfile,
    covers = ["B-46"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (
                    ".gitignore",
                    "node_modules/\n.next/\n.turbo/\n.nx/\ndist/\n",
                ),
            ],
        );
        let dirs = [
            "node_modules",
            ".next/cache",
            ".next/server",
            ".turbo",
            ".nx/cache",
            "dist",
        ];
        probe(&f, &dirs);
        let hit = probe(&f, &dirs);
        for d in ["node_modules", ".next/cache", ".turbo", ".nx/cache"] {
            assert!(kept(&hit, d), "{d} kept on a key match: {hit}");
        }
        for d in [".next/server", "dist"] {
            assert!(!kept(&hit, d), "{d} is a build output: {hit}");
        }
        wt_commit(&f, &[("package-lock.json", "v2")]);
        let miss = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&miss, d)), "{miss}");
    }
);

parity!(
    record_failed_run_does_not_vouch_for_the_cache,
    covers = ["B-45"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let bad = rec(
            &f,
            "unit",
            "mkdir -p node_modules && echo x > node_modules/dep.txt; exit 3",
        );
        assert_eq!(bad.exit, 3);
        let after = probe(&f, &["node_modules"]);
        assert!(!kept(&after, "node_modules"), "{after}");
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_cache_ignores_vendored_unsafe_and_uncached_ecosystems,
    covers = ["B-43", "B-48"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                (
                    ".gitignore",
                    "node_modules/\n.venv/\ntarget/\n.dart_tool/\n",
                ),
                ("vendor/foo/package-lock.json", "v1"),
                ("my dir/package-lock.json", "v1"),
                ("requirements.txt", "x"),
                ("Cargo.lock", "x"),
                ("go.sum", "x"),
                ("pubspec.lock", "x"),
            ],
        );
        let dirs = [
            "vendor/foo/node_modules",
            "my dir/node_modules",
            ".venv",
            "target",
            ".dart_tool",
        ];
        probe(&f, &dirs);
        let again = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&again, d)), "{again}");
    }
);

parity!(
    record_with_too_many_lockfiles_keeps_nothing,
    covers = ["B-47"],
    tags = [],
    {
        let f = fx();
        let mut files: Vec<(String, &str)> = (0..64)
            .map(|i| (format!("pkg{i}/package-lock.json"), "v1"))
            .collect();
        files.push(("package-lock.json".to_string(), "v1"));
        files.push((".gitignore".to_string(), "node_modules/\n"));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        wt_commit(&f, &refs);
        probe(&f, &["node_modules"]);
        let again = probe(&f, &["node_modules"]);
        assert!(!kept(&again, "node_modules"), "{again}");
    }
);

parity!(
    record_needs_a_git_checkout_on_the_box,
    covers = ["B-40"],
    tags = [],
    {
        let f = fx();
        wt_commit(&f, &[("a.txt", "a")]);
        crabbox::run_in(&f.w, &f.w.box_root().join("work"));
        let marker = f.w.box_root().join("command-ran");
        let o = rec(&f, "unit", &format!("touch '{}'", marker.display()));
        assert!(o.is_err());
        assert!(!marker.exists());
        assert_ne!(moor().runs(&f.w, &f.name)[0].exit, 0);
    }
);

fn start_again(f: &Fx) -> parity::adapter::Outcome<parity::adapter::TaskView> {
    moor().start(&f.w, &f.repo.path, &f.name, &StartOpts::default())
}

fn setup_cfg(steps: &[&str]) -> Option<Value> {
    Some(json!({ "setup": steps }))
}

parity!(
    provision_copies_and_links_into_the_worktree,
    covers = ["B-50"],
    tags = [],
    {
        let (w, repo) = prepare(
            Some(json!({
                "provision": [".env.local", "exists.txt", { "path": "cfg/secret.txt", "mode": "symlink" }],
                "setup": ["test -f .env.local && echo seen > .moor/prov-seen"],
            })),
            json!({}),
        );
        put(&repo.path, "exists.txt", "tracked");
        commit_all(&w, &repo.path, "add exists");
        put(&repo.path, "exists.txt", "host-copy");
        put(&repo.path, ".env.local", "TOKEN=1");
        put(&repo.path, "cfg/secret.txt", "s3");
        let (_name, wt, _slug) = start_task(&w, &repo, "prov");
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "TOKEN=1"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("exists.txt")).unwrap(),
            "tracked"
        );
        let link = wt.join("cfg/secret.txt");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(repo.path.join("cfg/secret.txt")).unwrap()
        );
        assert!(
            wt.join(".moor/prov-seen").exists(),
            "setup sees provisioned files"
        );
    }
);

parity!(
    provision_missing_source_warns_and_skips_setup,
    covers = ["B-51"],
    tags = [],
    {
        let (w, repo) = prepare(
            Some(
                json!({ "provision": ["missing-a", "missing-b"], "setup": ["touch .moor/setup-ran"] }),
            ),
            json!({}),
        );
        let o = moor().start(&w, &repo.path, &w.task_name("pv"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(!o.warnings.is_empty(), "the failure is a warning");
        let wt = PathBuf::from(o.ok().worktree.clone().unwrap());
        assert!(!wt.join(".moor/setup-ran").exists(), "setup is skipped");
        assert!(!wt.join("missing-a").exists());
    }
);

parity!(
    setup_runs_synced_on_the_box_with_a_log,
    covers = ["B-52", "B-55"],
    tags = [],
    {
        let cfg = json!({
            "run": { "env": { "SETUP_ENV": "from-run-env" } },
            "setup": ["echo step-one-ran", "echo $SETUP_ENV > .moor/env-seen"],
        });
        let f = fx_with(Some(cfg));
        let runs = user_runs(&f.w);
        assert_eq!(runs.len(), 1, "setup is one Driver run");
        assert!(!has(&runs[0], "--no-sync") && has(&runs[0], "--no-hydrate"));
        assert_eq!(runs[0].flag("--id").as_deref(), Some(f.slug.as_str()));
        let log = std::fs::read_to_string(f.wt.join(".moor/setup.log")).unwrap();
        assert!(log.contains("step-one-ran"), "{log}");
        assert_eq!(
            std::fs::read_to_string(f.wt.join(".moor/setup.rc"))
                .unwrap()
                .trim(),
            "0"
        );
        let seen = std::fs::read_to_string(f.wt.join(".moor/env-seen")).unwrap();
        assert_eq!(seen.trim(), "from-run-env");
    }
);

parity!(
    setup_failure_is_not_fatal_and_stops_later_steps,
    covers = ["B-53"],
    tags = [],
    {
        let (w, repo) = prepare(
            setup_cfg(&["echo before", "false", "echo after-the-failure"]),
            json!({}),
        );
        let o = moor().start(&w, &repo.path, &w.task_name("sf"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let t = o.ok();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(t.boxed.as_ref().unwrap().state, BoxState::Ready);
        assert!(
            !o.warnings.is_empty(),
            "setup failure surfaces as a warning"
        );
        let wt = PathBuf::from(t.worktree.clone().unwrap());
        let log = std::fs::read_to_string(wt.join(".moor/setup.log")).unwrap();
        assert!(
            log.contains("before") && !log.contains("after-the-failure"),
            "{log}"
        );
        assert_ne!(
            std::fs::read_to_string(wt.join(".moor/setup.rc"))
                .unwrap()
                .trim(),
            "0"
        );
    }
);

parity!(
    start_rerun_after_setup_failure_reuses_warmed_box,
    covers = ["B-54"],
    tags = [],
    {
        let f = fx_with(setup_cfg(&["false"]));
        let cfg = json!({ "setup": ["echo fixed > .moor/fixed"] });
        std::fs::write(f.repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        let again = start_again(&f);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(again.ok().state, TaskState::Working);
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
        assert_eq!(verb_calls(&f.w, "warmup").len(), 1);
    }
);

parity!(
    close_runs_teardown_before_release,
    covers = ["B-56"],
    tags = [],
    {
        let mark = |w: &World| w.box_root().join("teardown.mark");
        let probe_cfg = |w: &World| {
            Some(json!({ "teardown": [format!("echo bye > '{}'", mark(w).display())] }))
        };
        let (w, repo) = prepare(None, json!({}));
        std::fs::write(
            repo.path.join(".moor.yaml"),
            probe_cfg(&w).unwrap().to_string(),
        )
        .unwrap();
        let (name, _wt, _slug) = start_task(&w, &repo, "td");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(closed.ok().box_deleted);
        assert!(mark(&w).exists(), "teardown ran on the Box");
        assert_order(
            &w.all_calls(),
            &[("crabbox", &["run"]), ("crabbox", &["stop"])],
        );
        let last_run = user_runs(&w).pop().unwrap();
        assert!(!has(&last_run, "--no-sync"));
    }
);

parity!(
    close_teardown_failure_still_releases_the_box,
    covers = ["B-56"],
    tags = [],
    {
        let (w, repo) = prepare(Some(json!({ "teardown": ["false"] })), json!({}));
        let (name, _wt, _slug) = start_task(&w, &repo, "tf");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(closed.ok().box_deleted);
        assert_eq!(verb_calls(&w, "stop").len(), 1);
    }
);

parity!(
    close_skips_teardown_when_the_box_is_gone,
    covers = ["B-56"],
    tags = [],
    {
        let (w, repo) = prepare(None, json!({}));
        let mark = w.box_root().join("teardown.mark");
        let cfg = json!({ "teardown": [format!("echo bye > '{}'", mark.display())] });
        std::fs::write(repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        let (name, _wt, slug) = start_task(&w, &repo, "tg");
        crabbox::gone(&w, &slug);
        moor().list(&w, &repo.path).ok();
        moor().close(&w, &repo.path, &name, false);
        assert!(!mark.exists());
        assert!(verb_calls(&w, "run").is_empty());
    }
);

parity!(
    runs_that_sync_and_runs_that_do_not,
    covers = ["B-60"],
    tags = [],
    {
        let f = fx_with(Some(json!({ "setup": ["true"], "teardown": ["true"] })));
        go(&f, &["true"]);
        seed_box_file(&f, "rel.txt", "x");
        let cwd = host_dir(&f, "sync-cp");
        let cp = moor().cp(&f.w, &cwd, &f.name, "rel.txt", Path::new("o"));
        assert_eq!(cp.class, ExitClass::Ok, "{}", cp.stderr);
        let closed = moor().close(&f.w, &f.repo.path, &f.name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let all = verb_calls(&f.w, "run");
        let (probes, real): (Vec<_>, Vec<_>) = all
            .iter()
            .partition(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")));
        let synced = real.iter().filter(|c| !has(c, "--no-sync")).count();
        assert_eq!(synced, 3, "setup, run and teardown sync: {real:?}");
        assert_eq!(probes.len(), 1);
        assert!(has(probes[0], "--no-sync"));
        assert!(verb_calls(&f.w, "cp").iter().all(|c| !has(c, "--no-sync")));
    }
);

parity!(
    box_scratch_dirs_do_not_dirty_the_worktree,
    covers = ["B-65"],
    tags = [],
    {
        let f = fx();
        assert_eq!(
            go(
                &f,
                &["mkdir -p .moor .crabbox && echo x > .moor/a && echo y > .crabbox/b"]
            )
            .class,
            ExitClass::Ok
        );
        let status = git_out(&f.w, &f.wt, &["status", "--porcelain"]);
        assert_eq!(status, "", "worktree stays clean");
        let again = git_out(&f.w, &f.repo.path, &["status", "--porcelain"]);
        assert!(
            !again.contains(".moor/") && !again.contains(".crabbox/"),
            "{again}"
        );
    }
);

fn box_cred_world(secret: &str) -> (World, Repo) {
    let (w, repo) = prepare(
        Some(json!({ "box_credentials": { "TOK": { "env": "PARITY_TOK" } } })),
        json!({}),
    );
    let user = json!({
        "credentials": { "TOK": { "cmd": format!("printf %s {secret}") } },
        "drivers": { "crabbox": {} },
    });
    std::fs::write(w.config_path(), user.to_string()).unwrap();
    parity::fake::ssh::install(&w);
    let ssh = format!("{} fake-host\n", w.tool_path("ssh").display());
    w.prepend_rules(
        "crabbox",
        [
            parity::fake::Rule::on(&["ssh"]).stdout(ssh),
            parity::fake::Rule::on(&["run"])
                .exec("{post}")
                .rewrite("/tmp/moor-cred-", "{box}/tmp/moor-cred-"),
        ],
    );
    (w, repo)
}

parity!(
    run_delivers_box_credentials_over_ssh_and_cleans_up,
    covers = ["B-16"],
    tags = [],
    {
        let secret = "PARITY-SECRET-7788";
        let (mut w, repo) = box_cred_world(secret);
        w.watch_creds_in(Path::new("/tmp"));
        let (name, _wt, _slug) = start_task(&w, &repo, "bc");
        let o = moor().run(
            &w,
            &repo.path,
            &name,
            &["printf '%s' \"$PARITY_TOK\""],
            &RunOpts::default(),
        );
        assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
        assert_eq!(o.stdout, secret, "the Box command sees the credential");
        assert_order(
            &w.all_calls(),
            &[("crabbox", &["ssh"]), ("ssh", &[]), ("crabbox", &["run"])],
        );
        let leftovers: Vec<_> = std::fs::read_dir(w.box_root().join("tmp"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("moor-cred-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "credential dir removed: {leftovers:?}"
        );
        for c in w.calls("crabbox") {
            let hay = format!("{:?} {:?}", c.argv, c.env);
            assert!(
                !hay.contains(secret),
                "secret leaked into crabbox call: {hay}"
            );
        }
        w.assert_no_new_tmp_creds();
    }
);

parity!(
    run_with_missing_box_credential_source_fails_before_delivery,
    covers = ["B-16"],
    tags = [],
    {
        let (w, repo) = box_cred_world("x");
        let (name, _wt, _slug) = start_task(&w, &repo, "bm");
        std::fs::write(
            w.config_path(),
            json!({ "drivers": { "crabbox": {} } }).to_string(),
        )
        .unwrap();
        let o = moor().run(&w, &repo.path, &name, &["true"], &RunOpts::default());
        assert!(o.is_err());
        assert_never_called(&w.all_calls(), "ssh", &[]);
        assert!(user_runs(&w).is_empty());
    }
);

fn path_without_hash_tools(w: &World) -> PathBuf {
    let dir = w.box_root().join("nohash-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let skip = ["sha256sum", "shasum", "openssl"];
    for src in ["/usr/bin", "/bin"] {
        for e in std::fs::read_dir(src).into_iter().flatten().flatten() {
            let name = e.file_name();
            if skip.iter().any(|s| name == *s) {
                continue;
            }
            let _ = std::os::unix::fs::symlink(e.path(), dir.join(&name));
        }
    }
    dir
}

parity!(
    record_without_a_hash_tool_keeps_nothing_and_still_runs,
    covers = ["B-47"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let bin = path_without_hash_tools(&f.w);
        f.w.prepend_rules(
            "crabbox",
            [parity::fake::Rule::on(&["run"]).exec(format!("PATH='{}' {{post}}", bin.display()))],
        );
        probe(&f, &["node_modules"]);
        let again = probe(&f, &["node_modules"]);
        assert!(!kept(&again, "node_modules"), "{again}");
    }
);
