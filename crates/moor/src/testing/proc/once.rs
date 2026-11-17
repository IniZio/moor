//! One-shot scripted replies.

use moor_core::seam::proc::Tool;

use super::{Reply, Rule, ScriptedProc};

impl ScriptedProc {
    pub(super) fn push_rule(&self, tool: Tool, prefix: &[&str], reply: Reply, once: bool) -> &Self {
        if let Ok(mut r) = self.rules.lock() {
            r.push(rule(tool, prefix, reply, once));
        }
        self
    }
}

fn rule(tool: Tool, prefix: &[&str], reply: Reply, once: bool) -> Rule {
    Rule {
        tool,
        prefix: prefix.iter().map(|s| (*s).to_owned()).collect(),
        reply,
        once,
    }
}
