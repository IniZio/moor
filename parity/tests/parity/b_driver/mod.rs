use parity::adapter::{BoxState, EventKind, ExitClass, StartOpts, TaskState, TaskView};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{herdr, Call};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) fn write_user(w: &World, provider: &str, crabbox_keys: Value, enforce_egress: bool) {
    let mut b = json!({ "provider": provider });
    if enforce_egress {
        b["enforce_egress"] = json!(true);
    }
    let doc = json!({ "box": b, "drivers": { "crabbox": crabbox_keys } });
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

pub(super) fn write_repo(repo: &Repo, doc: Value) {
    std::fs::write(repo.path.join(".moor.yaml"), doc.to_string()).expect("write repo config");
}

pub(super) fn verb_calls(w: &World, verb: &str) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
        .collect()
}

pub(super) fn env_of(c: &Call, key: &str) -> Option<String> {
    c.env.get(key).cloned()
}

pub(super) fn setup(preset: Preset, crabbox_keys: Value, enforce: bool) -> (World, Repo) {
    let w = world();
    let repo = Repo::create(&w, "widget");
    write_user(&w, preset.provider(), crabbox_keys, enforce);
    crabbox::install_preset(&w, preset);
    herdr::install(&w);
    (w, repo)
}

pub(super) fn started(w: &World, repo: &Repo, name: &str) -> TaskView {
    let m = moor();
    let o = m.start(w, &repo.path, name, &StartOpts::default());
    assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    o.ok().clone()
}

pub(super) fn assert_ready_handle(task: &TaskView, provider: &str, slug: &str) {
    assert_eq!(task.state, TaskState::Working);
    let b = task.boxed.as_ref().expect("box present");
    assert_eq!(b.state, BoxState::Ready);
    assert_eq!(
        b.handle.as_deref(),
        Some(format!("{provider}/{slug}").as_str())
    );
}

pub(super) fn assert_warmup(w: &World, provider: &str, slug: &str, class: &str) -> Call {
    let warm = verb_calls(w, "warmup");
    assert_eq!(warm.len(), 1, "one warmup");
    let c = warm[0].clone();
    assert_eq!(c.flag("--provider").as_deref(), Some(provider));
    assert_eq!(c.flag("--slug").as_deref(), Some(slug));
    assert_eq!(c.flag("--class").as_deref(), Some(class));
    c
}

pub(super) fn assert_run_targets(w: &World, provider: &str, slug: &str) {
    let runs = verb_calls(w, "run");
    for r in &runs {
        assert_eq!(r.flag("--provider").as_deref(), Some(provider));
        assert_eq!(r.flag("--id").as_deref(), Some(slug));
    }
}

mod failure;
mod lifecycle;
