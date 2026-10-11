use std::path::{Path, PathBuf};

use parity::adapter::{EventKind, ExitClass, RunOpts, StartOpts};
use parity::fake::{crabbox, herdr, Call};
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
    pub(super) name: String,
    pub(super) wt: PathBuf,
    pub(super) slug: String,
}

pub(super) fn write_user(w: &World, crabbox_keys: Value) {
    let doc = json!({ "drivers": { "crabbox": crabbox_keys } });
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

pub(super) fn commit_all(w: &World, dir: &Path, msg: &str) {
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

pub(super) fn git_out(w: &World, dir: &Path, args: &[&str]) -> String {
    let o = w
        .command("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

pub(super) fn prepare(repo_cfg: Option<Value>, crabbox_keys: Value) -> (World, Repo) {
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

pub(super) fn start_task(w: &World, repo: &Repo, base: &str) -> (String, PathBuf, String) {
    let name = w.task_name(base);
    let o = moor().start(w, &repo.path, &name, &StartOpts::default());
    assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    let wt = PathBuf::from(o.ok().worktree.clone().expect("worktree"));
    let slug = crabbox::slug(w).expect("warmup slug");
    (name, wt, slug)
}

pub(super) fn fx_with(repo_cfg: Option<Value>) -> Fx {
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

pub(super) fn fx() -> Fx {
    fx_with(None)
}

pub(super) fn verb_calls(w: &World, verb: &str) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
        .collect()
}

pub(super) fn user_runs(w: &World) -> Vec<Call> {
    verb_calls(w, "run")
        .into_iter()
        .filter(|c| !c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
        .collect()
}

pub(super) fn has(c: &Call, flag: &str) -> bool {
    c.argv.iter().any(|a| a == flag)
}

pub(super) fn go(f: &Fx, argv: &[&str]) -> parity::adapter::Outcome<parity::adapter::RunResult> {
    moor().run(&f.w, &f.repo.path, &f.name, argv, &RunOpts::default())
}

pub(super) fn ran_events(w: &World, name: &str) -> usize {
    moor()
        .events(w, name)
        .iter()
        .filter(|e| e.kind == EventKind::Ran)
        .count()
}

pub(super) fn seed_box_file(f: &Fx, rel: &str, body: &str) -> PathBuf {
    let p = f.w.box_root().join("work").join(rel);
    std::fs::write(&p, body).unwrap();
    p
}

pub(super) fn host_dir(f: &Fx, name: &str) -> PathBuf {
    let d = f.w.tmp_dir().join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(super) fn put(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

pub(super) fn wt_commit(f: &Fx, files: &[(&str, &str)]) -> String {
    for (rel, body) in files {
        put(&f.wt, rel, body);
    }
    commit_all(&f.w, &f.wt, "parity change");
    git_out(&f.w, &f.wt, &["rev-parse", "HEAD"])
}

pub(super) fn rec(
    f: &Fx,
    name: &str,
    cmd: &str,
) -> parity::adapter::Outcome<parity::adapter::RunResult> {
    let opts = RunOpts {
        record: Some(name.to_string()),
        ..Default::default()
    };
    moor().run(&f.w, &f.repo.path, &f.name, &[cmd], &opts)
}

mod cache;
mod cp;
mod gate;
mod run;
mod setup;
mod sync;
