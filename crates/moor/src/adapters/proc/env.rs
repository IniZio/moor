//! Environment snapshot handed to spawned children.

use std::ffi::OsString;
use std::process::Command;

use moor_core::seam::proc::{Tool, BASE_ENV};

use super::ToolPaths;
use crate::env::Env;

/// Program for `tool`: `MOOR_<TOOL>_BIN`, then config, then the default name.
pub(super) fn resolve(env: &Env, tools: &ToolPaths, tool: Tool) -> OsString {
    if let Some(v) = env.get(tool.bin_var()).filter(|v| !v.is_empty()) {
        return v.into();
    }
    tools
        .get(tool)
        .map_or_else(|| tool.program().into(), |p| p.clone().into_os_string())
}

/// Name and value pairs.
type Vars = Vec<(String, String)>;

/// Variables captured at construction.
#[derive(Debug)]
pub(super) struct Snapshot {
    /// Variables every child inherits.
    base: Vars,
}

impl Snapshot {
    /// Captures the base variables from `env`.
    pub(super) fn new(env: &Env) -> Self {
        let base = BASE_ENV
            .iter()
            .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
            .collect();
        Self { base }
    }

    /// Sets the variables every child inherits.
    pub(super) fn apply(&self, cmd: &mut Command) {
        cmd.envs(self.base.iter().map(|(k, v)| (k, v)));
    }
}
