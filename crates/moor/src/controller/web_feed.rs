//! Wires the web UI to the controller: its `Feed` over `Moor`, and the token file.

use std::fs::File;
use std::io::{self, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use moor_core::controller::ServerFrame;
use moor_core::view::TaskView;

use super::web::{Feed, FeedFactory};
use crate::env::Env;
use crate::Moor;

/// `Feed` answered by a freshly wired [`Moor`].
struct MoorFeed(Moor);

impl Feed for MoorFeed {
    fn tasks(&self) -> Result<Vec<TaskView>, String> {
        self.0.open_tasks()
    }

    fn events_after(&self, after: i64, limit: usize) -> Result<Vec<ServerFrame>, String> {
        self.0.events_after(after, limit)
    }
}

/// Factory wiring a new `Moor` from `env` per call, since `Moor` is not `Send`.
pub(super) fn factory(env: &Env) -> FeedFactory {
    let env = env.clone();
    Arc::new(move || {
        let moor = Moor::from_env(&env).map_err(|e| e.error.to_string())?;
        Ok(Box::new(MoorFeed(moor)) as Box<dyn Feed>)
    })
}

/// Reads the bearer token from `path`, which must be owner-only and non-empty.
pub(super) fn read_token(path: &Path) -> Result<String, String> {
    let shown = path.display();
    let fail = |why: &dyn std::fmt::Display| format!("web token file {shown}: {why}");
    let read = || -> io::Result<Result<String, &'static str>> {
        let mut file = File::open(path)?;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Ok(Err("must be mode 0600"));
        }
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let token = text.trim_end();
        Ok(if token.is_empty() {
            Err("is empty")
        } else {
            Ok(token.to_owned())
        })
    };
    match read() {
        Ok(Ok(token)) => Ok(token),
        Ok(Err(why)) => Err(fail(&why)),
        Err(e) => Err(fail(&e)),
    }
}

#[cfg(test)]
mod tests;
