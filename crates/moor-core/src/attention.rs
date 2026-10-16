//! Open rate-limit and credential problems derived from a Task's events.

use serde_json::Value;

use crate::view::AttentionView;
use std::collections::BTreeMap;

/// An open problem with a credential rule's account that the user must see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit {
    /// What kind of problem.
    pub kind: LimitKind,
    /// Credential rule name.
    pub rule: String,
    /// Upstream HTTP status.
    pub status: u16,
    /// When the limit resets, if known.
    pub resets_at: Option<String>,
}

/// Kind of an open [`Limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// The upstream answered 429.
    RateLimited,
    /// The upstream rejected the credential.
    Rejected,
}

impl LimitKind {
    /// The event kind that opens this limit.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "agent_rate_limited",
            Self::Rejected => "proxy_credential_rejected",
        }
    }
}

impl Limit {
    /// One-line summary, e.g. `Claude account rate limited (429), resets <time>`.
    #[must_use]
    pub fn message(&self) -> String {
        let mut chars = self.rule.chars();
        let name: String = chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        match self.kind {
            LimitKind::RateLimited => {
                let base = format!("{name} account rate limited ({})", self.status);
                match &self.resets_at {
                    Some(t) => format!("{base}, resets {t}"),
                    None => base,
                }
            }
            LimitKind::Rejected => format!("{name} credential rejected ({})", self.status),
        }
    }
}

/// Open limits from a Task's events in insertion order, given as (kind, payload) pairs.
///
/// Per rule, the latest of `agent_rate_limited`, `proxy_credential_rejected` and
/// `proxy_credential_recovered` wins; a rule whose latest is recovered has no open
/// limit. Other kinds are ignored. The result is sorted by rule name.
#[must_use]
pub fn open_limits<'a>(events: impl IntoIterator<Item = (&'a str, &'a Value)>) -> Vec<Limit> {
    let mut latest: BTreeMap<String, Option<Limit>> = BTreeMap::new();
    for (kind, payload) in events {
        let kind = match kind {
            "agent_rate_limited" => Some(LimitKind::RateLimited),
            "proxy_credential_rejected" => Some(LimitKind::Rejected),
            "proxy_credential_recovered" => None,
            _ => continue,
        };
        let Some(rule) = payload.get("rule").and_then(Value::as_str) else {
            continue;
        };
        let limit = kind.map(|kind| Limit {
            kind,
            rule: rule.to_owned(),
            status: payload
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or_default(),
            resets_at: payload
                .get("resets_at")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
        latest.insert(rule.to_owned(), limit);
    }
    latest.into_values().flatten().collect()
}

impl From<&Limit> for AttentionView {
    fn from(l: &Limit) -> Self {
        Self {
            kind: l.kind.as_str().to_owned(),
            rule: l.rule.clone(),
            status: l.status,
            resets_at: l.resets_at.clone(),
            message: l.message(),
        }
    }
}

#[cfg(test)]
mod tests;
