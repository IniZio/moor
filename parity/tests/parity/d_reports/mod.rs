use std::path::{Path, PathBuf};

use parity::adapter::{
    BoxState, CloseView, ExitClass, Moor, Outcome, ReportReq, ReportRow, StartOpts, TaskState,
    TaskView,
};
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::parity;
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

pub(super) fn fx_with_config(config: Option<Value>) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    match config {
        Some(doc) => std::fs::write(w.config_path(), doc.to_string()).expect("write config"),
        None => UserConfig::default().write(&w),
    }
    crabbox::install(&w);
    crabbox::cp(&w);
    herdr::install(&w);
    claude::install(&w);
    Fx { w, repo, m: moor() }
}

pub(super) fn fx() -> Fx {
    fx_with_config(None)
}

impl Fx {
    pub(super) fn name(&self, base: &str) -> String {
        self.w.task_name(base)
    }

    pub(super) fn start_with(&self, name: &str, goal: Option<&str>) -> TaskView {
        let opts = StartOpts {
            goal: goal.map(str::to_string),
            ..Default::default()
        };
        let o = self.m.start(&self.w, &self.repo.path, name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        if let Some(slug) = crabbox::slug(&self.w) {
            crabbox::inventory(&self.w, &[&slug]);
        }
        o.ok().clone()
    }

    pub(super) fn start(&self, base: &str) -> String {
        let name = self.name(base);
        self.start_with(&name, None);
        name
    }

    pub(super) fn src(&self, file: &str, body: &str) -> PathBuf {
        let p = self.w.tmp_dir().join(file);
        std::fs::write(&p, body).expect("write source");
        p
    }

    pub(super) fn report(&self, name: &str, req: &ReportReq) -> Outcome<ReportRow> {
        self.m.report(&self.w, &self.repo.path, name, req)
    }

    pub(super) fn reports(&self, name: &str) -> Outcome<Vec<ReportRow>> {
        self.m.reports(&self.w, &self.repo.path, name)
    }

    pub(super) fn row(&self, name: &str) -> TaskView {
        let o = self.m.list(&self.w, &self.repo.path);
        o.ok()
            .iter()
            .rfind(|t| t.name == name)
            .expect("task listed")
            .clone()
    }

    pub(super) fn close(&self, name: &str) -> CloseView {
        let o = self.m.close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }

    pub(super) fn stored_files(&self) -> Vec<PathBuf> {
        files_under(&self.w.state_dir().join("reports"))
    }

    pub(super) fn assert_stored_under_state(&self, copy: &Path) {
        let root = std::fs::canonicalize(self.w.state_dir().join("reports")).expect("reports dir");
        let real = std::fs::canonicalize(copy).expect("copy exists");
        assert!(
            real.starts_with(&root),
            "{} not under state",
            real.display()
        );
    }

    pub(super) fn bg_call(&self) -> Call {
        self.w
            .calls("claude")
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a == "--bg"))
            .expect("claude --bg dispatched")
    }
}

pub(super) fn files_under(dir: &Path) -> Vec<PathBuf> {
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
    out.sort();
    out
}

pub(super) fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

mod briefs;
mod reports;
