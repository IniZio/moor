use std::path::Path;

use parity::adapter::{
    BoxState, CloseView, EventKind, ExitClass, Moor, Outcome, StartOpts, TaskState, TaskView,
};
use parity::fake::{crabbox, herdr, Call, Rule};
use parity::world::config::UserConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
    pub(super) m: Box<dyn Moor>,
}

pub(super) fn fx_with(user: UserConfig) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    user.write(&w);
    crabbox::install(&w);
    herdr::install(&w);
    follow_warmups(&w);
    crabbox::ssh_channel(&w);
    crabbox::tunnel(&w);
    Fx { w, repo, m: moor() }
}

pub(super) fn follow_warmups(w: &World) {
    let script = concat!(
        "printf '['; sep=''; ",
        "for s in $(grep -o '\"--slug\",\"[^\"]*\"' {root}/fakes/crabbox.jsonl | cut -d'\"' -f4); do ",
        "printf '%s{\"name\":\"%s\"}' \"$sep\" \"$s\"; sep=','; done; printf ']'"
    );
    w.prepend_rules("crabbox", [Rule::on(&["list"]).exec(script)]);
}

pub(super) fn fx() -> Fx {
    fx_with(UserConfig::default())
}

pub(super) fn write_json(path: &Path, v: Value) {
    std::fs::write(path, v.to_string()).expect("write config");
}

impl Fx {
    pub(super) fn name(&self, base: &str) -> String {
        self.w.task_name(base)
    }
    pub(super) fn try_start(&self, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
        self.m.start(&self.w, &self.repo.path, name, opts)
    }
    pub(super) fn start_with(&self, name: &str, opts: &StartOpts) -> TaskView {
        let o = self.try_start(name, opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    pub(super) fn start(&self, name: &str) -> TaskView {
        self.start_with(name, &StartOpts::default())
    }
    pub(super) fn goal(&self, name: &str) -> TaskView {
        let opts = StartOpts {
            goal: Some("say hi".into()),
            ..Default::default()
        };
        self.start_with(name, &opts)
    }
    pub(super) fn list(&self) -> Vec<TaskView> {
        let o = self.m.list(&self.w, &self.repo.path);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    pub(super) fn rows(&self, name: &str) -> Vec<TaskView> {
        self.list().into_iter().filter(|t| t.name == name).collect()
    }
    pub(super) fn row(&self, name: &str) -> TaskView {
        let mut rows = self.rows(name);
        assert!(!rows.is_empty(), "task {name} not listed");
        rows.remove(rows.len() - 1)
    }
    pub(super) fn try_close(&self, name: &str) -> Outcome<CloseView> {
        self.m.close(&self.w, &self.repo.path, name, false)
    }
    pub(super) fn close(&self, name: &str) -> CloseView {
        let o = self.try_close(name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }
    pub(super) fn kinds(&self, name: &str) -> Vec<EventKind> {
        self.m
            .events(&self.w, name)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }
    pub(super) fn verb(&self, tool: &str, verb: &str) -> Vec<Call> {
        self.w
            .calls(tool)
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
            .collect()
    }
    pub(super) fn n(&self, tool: &str, verb: &str) -> usize {
        self.verb(tool, verb).len()
    }
    pub(super) fn repo_cfg(&self, v: Value) {
        write_json(&self.repo.path.join(".moor.yaml"), v);
    }
    pub(super) fn sync_inventory(&self) {
        let slugs: Vec<String> = self
            .verb("crabbox", "warmup")
            .iter()
            .filter_map(|c| c.flag("--slug"))
            .collect();
        let refs: Vec<&str> = slugs.iter().map(String::as_str).collect();
        crabbox::inventory(&self.w, &refs);
    }
}

pub(super) fn box_state(t: &TaskView) -> BoxState {
    t.boxed.as_ref().expect("box present").state.clone()
}

pub(super) fn git_in(f: &Fx, dir: &Path, args: &[&str]) -> String {
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

pub(super) fn bound_worktree(f: &Fx, branch: &str) -> std::path::PathBuf {
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

pub(super) fn bind_opts(wt: &Path) -> StartOpts {
    StartOpts {
        worktree: Some(wt.to_path_buf()),
        ..Default::default()
    }
}

pub(super) fn fx_preset(preset: crabbox::Preset, user: UserConfig) -> Fx {
    let f = fx_with(user.provider(preset.provider()));
    crabbox::install_preset(&f.w, preset);
    f
}

pub(super) fn call_seq(calls: &[Call], pick: impl Fn(&Call) -> bool) -> u64 {
    calls
        .iter()
        .find(|c| pick(c))
        .map_or_else(|| panic!("call missing"), |c| c.seq)
}

pub(super) fn is_verb(c: &Call, tool: &str, verb: &str) -> bool {
    c.tool == tool && c.argv.first().map(String::as_str) == Some(verb)
}

pub(super) fn failed_acquire(f: &Fx, name: &str) -> TaskView {
    crabbox::fail_quota(&f.w);
    let o = f.try_start(name, &StartOpts::default());
    assert!(o.class.is_err());
    let row = f.row(name);
    assert_eq!(row.state, TaskState::SetupFailed);
    assert!(f.kinds(name).contains(&EventKind::BoxFailed));
    row
}

pub(super) fn dotfiles_fx() -> (Fx, std::path::PathBuf) {
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
    f.repo_cfg(json!({ "setup": ["echo setup-marker"] }));
    crabbox::ssh_wrapper(&f.w);
    parity::fake::ssh::install(&f.w);
    let home = f.w.box_root().join("home");
    (f, home)
}

pub(super) fn run_with(f: &Fx, needle: &str) -> Vec<Call> {
    f.verb("crabbox", "run")
        .into_iter()
        .filter(|c| c.argv.iter().any(|a| a.contains(needle)))
        .collect()
}

pub(super) fn commit_in(f: &Fx, wt: &Path, file: &str) -> String {
    std::fs::write(wt.join(file), "w").unwrap();
    git_in(f, wt, &["add", file]);
    git_in(f, wt, &["commit", "-q", "-m", "work"]);
    git_in(f, wt, &["rev-parse", "HEAD"])
}

pub(super) fn closed_task(f: &Fx, base: &str) -> (String, std::path::PathBuf) {
    let name = f.name(base);
    let t = f.start(&name);
    f.close(&name);
    (name, std::path::PathBuf::from(t.worktree.unwrap()))
}

pub(super) fn branch_exists(f: &Fx, name: &str) -> bool {
    !git_in(
        f,
        &f.repo.path,
        &["branch", "--list", &format!("moor/{name}")],
    )
    .is_empty()
}

pub(super) fn remove(
    f: &Fx,
    name: &str,
    discard: bool,
) -> parity::adapter::Outcome<parity::adapter::RemoveView> {
    f.m.remove(&f.w, &f.repo.path, name, discard)
}

pub(super) fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
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

pub(super) fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}
