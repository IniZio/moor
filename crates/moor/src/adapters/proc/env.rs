//! Environment snapshot handed to spawned children.

use std::process::Command;

use moor_core::seam::proc::{Tool, BASE_ENV, CRABBOX_ENV, CRABBOX_ENV_PREFIX};

use crate::env::Env;

/// Name and value pairs.
type Vars = Vec<(String, String)>;

/// Variables captured at construction.
#[derive(Debug)]
pub(super) struct Snapshot {
    /// Variables every child inherits.
    base: Vars,
    /// Extra variables only crabbox inherits.
    crabbox: Vars,
}

impl Snapshot {
    /// Captures base and crabbox variables from `env`.
    pub(super) fn new(env: &Env) -> Self {
        let base = BASE_ENV
            .iter()
            .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
            .collect();
        let crabbox = CRABBOX_ENV
            .iter()
            .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
            .chain(
                env.with_prefix(CRABBOX_ENV_PREFIX)
                    .map(|(k, v)| (k.to_owned(), v.to_owned())),
            )
            .collect();
        Self { base, crabbox }
    }

    /// Sets the variables `tool` inherits.
    pub(super) fn apply(&self, cmd: &mut Command, tool: Tool) {
        cmd.envs(self.base.iter().map(|(k, v)| (k, v)));
        if tool == Tool::Crabbox {
            cmd.envs(self.crabbox.iter().map(|(k, v)| (k, v)));
        }
    }
}
