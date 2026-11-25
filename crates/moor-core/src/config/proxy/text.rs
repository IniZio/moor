//! A rules text: the `rules:` list a Box's Proxy follows, written in full and with no rule
//! names.
//!
//! Each rule still has an id inside moor, for its Proxy routes, the `rule` of its events and
//! the per-run grants that add allow entries to it. The id comes from the rule's first host,
//! so it never shows in the text.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::ToolRule;
use crate::config::{parse_strict, ConfigError};

/// The ids the default rules had when they were named, kept so events, Proxy routes and
/// stored per-run grants that name them still match.
const KEPT_IDS: &[(&str, &str)] = &[
    ("api.anthropic.com", "claude"),
    ("api.github.com", "github-api"),
    ("github.com", "github-git"),
    ("uploads.github.com", "github-uploads"),
];

/// A rules text.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RulesText {
    /// The rules, in the order written.
    rules: Vec<ToolRule>,
}

/// The id of `rule`: the kept id of its first host, or that host.
fn rule_id(rule: &ToolRule) -> String {
    let first = rule
        .hosts
        .first()
        .map(|h| h.trim().to_ascii_lowercase())
        .unwrap_or_default();
    KEPT_IDS
        .iter()
        .find(|(host, _)| *host == first)
        .map_or(first, |(_, id)| (*id).to_owned())
}

/// Parses and checks a rules text; the rules come back by id.
///
/// # Errors
/// The first invalid rule, or a host two rules list, by its place in the list.
pub fn parse_rules_text(file: &str, text: &str) -> Result<BTreeMap<String, ToolRule>, ConfigError> {
    let parsed: RulesText = parse_strict(file, text)?;
    let mut owner: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (i, rule) in parsed.rules.into_iter().enumerate() {
        let at = format!("rules[{i}]");
        rule.validate(&at).map_err(|e| e.in_file(file))?;
        for (j, host) in rule.hosts.iter().enumerate() {
            if let Some(k) = owner.insert(host.trim().to_ascii_lowercase(), i) {
                return Err(ConfigError::new(
                    format!("{at}.hosts[{j}]"),
                    format!("{host} is also listed by rules[{k}]; a host belongs to one rule"),
                )
                .in_file(file));
            }
        }
        out.insert(rule_id(&rule), rule);
    }
    Ok(out)
}
