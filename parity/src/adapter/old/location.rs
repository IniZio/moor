use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use super::registry::open_registry;
use crate::adapter::StartOpts;
use crate::world::World;

type Saved = Vec<(PathBuf, Option<Vec<u8>>)>;

/// Scoped pin: restores every touched file to its prior bytes (or absence) on drop.
pub(super) struct Pin {
    files: Saved,
    excludes: Saved,
    added: Vec<(PathBuf, Vec<u8>)>,
    made_dirs: Vec<PathBuf>,
    pinned: Vec<PathBuf>,
}

/// Old defaults the agent to the host; pin it to the Box before start/env.
pub(super) fn pin_start(w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Pin {
    let mut dirs = vec![cwd.to_path_buf()];
    dirs.extend(opts.worktree.clone());
    dirs.extend(bound_worktree(w, name));
    Pin::new(&dirs)
}

pub(super) fn pin_env(cwd: &Path) -> Pin {
    Pin::new(&[cwd.to_path_buf()])
}

impl Pin {
    fn new(dirs: &[PathBuf]) -> Pin {
        let mut pin = Pin {
            files: vec![],
            excludes: vec![],
            added: vec![],
            made_dirs: vec![],
            pinned: vec![],
        };
        for dir in dirs.iter().filter(|d| d.is_dir()) {
            if pin.exclude_config(dir) {
                let path = dir.join(".moor.yaml");
                pin.save(&path, false);
                merge_box_location(&path);
                pin.pinned.push(dir.clone());
            }
        }
        pin
    }

    fn save(&mut self, path: &Path, exclude: bool) {
        let list = if exclude {
            &mut self.excludes
        } else {
            &mut self.files
        };
        if !list.iter().any(|(p, _)| p == path) {
            list.push((path.to_path_buf(), std::fs::read(path).ok()));
        }
    }

    /// A worktree created during start: drop a shim-copied `.moor.yaml` there.
    pub(super) fn settle_worktree(&self, w: &World, name: &str) {
        let Some(wt) = bound_worktree(w, name).filter(|d| !self.pinned.contains(d)) else {
            return;
        };
        let file = wt.join(".moor.yaml");
        let tracked = Command::new("git")
            .args(["ls-files", "--error-unmatch", ".moor.yaml"])
            .current_dir(&wt)
            .output()
            .is_ok_and(|o| o.status.success());
        let ours = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v == json!({"agent": {"location": "box"}}));
        if !tracked && ours {
            let _ = std::fs::remove_file(&file);
        }
    }

    fn exclude_config(&mut self, dir: &Path) -> bool {
        let out = Command::new("git")
            .args(["rev-parse", "--git-path", "info/exclude"])
            .current_dir(dir)
            .output();
        let Ok(out) = out else { return false };
        if !out.status.success() {
            return false;
        }
        let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let file = dir.join(rel);
        self.save(&file, true);
        let old = std::fs::read_to_string(&file).unwrap_or_default();
        if old.lines().any(|l| l.trim() == ".moor.yaml") {
            return true;
        }
        if let Some(parent) = file.parent().filter(|p| !p.exists()) {
            std::fs::create_dir_all(parent).expect("create info dir");
            self.made_dirs.push(parent.to_path_buf());
        }
        let sep = if old.is_empty() || old.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let add = format!("{sep}.moor.yaml\n");
        std::fs::write(&file, format!("{old}{add}")).expect("write info/exclude");
        self.added.push((file, add.into_bytes()));
        true
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        for (path, prior) in &self.excludes {
            let add = self.added.iter().find(|(p, _)| p == path);
            let Some((_, add)) = add else { continue };
            let base = prior.clone().unwrap_or_default();
            let cur = std::fs::read(path).unwrap_or_default();
            let head = [base.as_slice(), add.as_slice()].concat();
            if let Some(tail) = cur.strip_prefix(head.as_slice()) {
                let kept = [base.as_slice(), tail].concat();
                let _ = if prior.is_none() && kept.is_empty() {
                    std::fs::remove_file(path)
                } else {
                    std::fs::write(path, kept)
                };
            }
        }
        for (path, prior) in &self.files {
            let _ = match prior {
                Some(bytes) => std::fs::write(path, bytes),
                None => std::fs::remove_file(path).or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                }),
            };
        }
        for dir in self.made_dirs.iter().rev() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

fn bound_worktree(w: &World, name: &str) -> Option<PathBuf> {
    let conn = open_registry(w)?;
    let wt: String = conn
        .query_row("SELECT worktree FROM tasks WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .ok()?;
    (!wt.is_empty()).then(|| PathBuf::from(wt))
}

fn merge_box_location(path: &Path) {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) => parse_config(&s, path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => panic!("read {}: {e}", path.display()),
    };
    let root = doc
        .as_object_mut()
        .unwrap_or_else(|| panic!("{} is not an object", path.display()));
    let agent = root.entry("agent").or_insert_with(|| json!({}));
    agent
        .as_object_mut()
        .unwrap_or_else(|| panic!("{}: `agent` is not an object", path.display()))
        .insert("location".into(), json!("box"));
    std::fs::write(path, doc.to_string())
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn parse_config(text: &str, path: &Path) -> Value {
    if text.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

pub(super) fn registered(w: &World, name: &str) -> bool {
    open_registry(w).is_some_and(|c| {
        c.query_row("SELECT 1 FROM tasks WHERE name = ?1", [name], |_| Ok(()))
            .is_ok()
    })
}
