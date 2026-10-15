//! One-shot scripted replies.

use moor_core::seam::proc::Tool;

use super::{Reply, Rule, ScriptedProc};

impl ScriptedProc {
    /// Like [`Self::on`], but the reply answers one call only; later calls fall to the next rule.
    pub(crate) fn once(&self, tool: Tool, prefix: &[&str], reply: Reply) -> &Self {
        self.push_rule(tool, prefix, reply, true)
    }

    /// Like [`Self::on`], but takes precedence over every rule registered so far.
    pub(crate) fn before(&self, tool: Tool, prefix: &[&str], reply: Reply) -> &Self {
        if let Ok(mut r) = self.rules.lock() {
            r.insert(0, rule(tool, prefix, reply, false));
        }
        self
    }

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
