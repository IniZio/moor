use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::world::World;

pub(super) fn open_registry(w: &World) -> Option<Connection> {
    let db = w.state_dir().join("registry.db");
    db.exists().then(|| {
        Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open {}: {e}", db.display()))
    })
}

pub(super) fn shift_ts(conn: &Connection, sql: &str, secs: i64, name: &str) {
    conn.execute(sql, rusqlite::params![secs, name])
        .unwrap_or_else(|e| panic!("age registry: {e}"));
}

pub(super) fn shift_hook_cache(state: &Path, secs: i64) {
    let mut stack = vec![state.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_cache = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("agent-state-last.json"));
            if !is_cache {
                continue;
            }
            let Ok(mut doc) = serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap())
            else {
                continue;
            };
            if let Some(at) = doc["read_at"].as_i64() {
                doc["read_at"] = Value::from(at - secs);
                std::fs::write(&path, doc.to_string()).expect("rewrite hook cache");
            }
        }
    }
}

pub(super) fn db_path(w: &World) -> PathBuf {
    w.state_dir().join("registry.db")
}
