//! Text form of `profile-check`.

use std::fmt::Write as _;

use moor_core::view::user::{AgentInstalls, ProfileCheckValue, ProfileResolved};
use serde_json::Value;

/// What the check read, what it found, and what failed.
pub(super) fn text(value: &Value) -> Option<String> {
    let v: ProfileCheckValue = serde_json::from_value(value.clone()).ok()?;
    let mut out = String::new();
    match &v.source {
        Some(source) => writeln!(out, "profile: {source}").ok()?,
        None => out.push_str("no profile set\n"),
    }
    if let Some(r) = &v.resolved {
        resolved(&mut out, r)?;
    }
    for p in &v.failures {
        writeln!(out, "failed {:?}: {}", p.step, p.detail).ok()?;
    }
    if v.source.is_some() && v.failures.is_empty() {
        out.push_str("ok\n");
    }
    if let Some(l) = &v.last_failed {
        let (step, on, why) = (l.step, &l.box_name, &l.detail);
        writeln!(out, "last failure: {step:?} on {on}: {why}").ok()?;
    }
    Some(out)
}

/// The commit, `setup.sh` and what each agent installs.
fn resolved(out: &mut String, r: &ProfileResolved) -> Option<()> {
    writeln!(out, "resolved: {}", r.sha).ok()?;
    writeln!(out, "setup.sh: {}", if r.setup { "yes" } else { "no" }).ok()?;
    installs(out, "claude", &r.claude)?;
    installs(out, "codex", &r.codex)?;
    for why in &r.unresolved {
        writeln!(out, "unresolved: {why}").ok()?;
    }
    Some(())
}

/// One agent's installs, when it has any.
fn installs(out: &mut String, agent: &str, i: &AgentInstalls) -> Option<()> {
    if i.model.is_none() && i.marketplaces.is_empty() && i.plugins.is_empty() {
        return Some(());
    }
    writeln!(out, "{agent}:").ok()?;
    if let Some(model) = &i.model {
        writeln!(out, "  model: {model}").ok()?;
    }
    for m in &i.marketplaces {
        writeln!(out, "  marketplace {}: {}", m.name, m.repo).ok()?;
    }
    for p in &i.plugins {
        writeln!(out, "  plugin: {p}").ok()?;
    }
    Some(())
}
