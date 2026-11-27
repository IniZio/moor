//! The effects of a `ReadingView`: each as shown, whether a slice-1 allow still stands, and the
//! allow entries the Proxy gets.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{EffectKind, FromView};
use crate::reading::{Effect, Origin, ProxyAllow};

/// One thing moor holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectView {
    /// Its id, such as `e1`.
    pub id: String,
    /// What it is.
    pub kind: EffectKind,
    /// What the file or the gate list shows: an allow's entry, a gate's or outcome's name.
    pub line: String,
    /// Equal across readings for the same effect.
    pub key: String,
    /// The phrases that hold it.
    pub phrases: Vec<String>,
    /// Where an allow came from; none for a gate or outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<FromView>,
}

/// Whether `e` still stands: a slice-1 allow effect needs its entry in `allow`.
pub(super) fn listed(e: &Effect, allow: &ProxyAllow) -> bool {
    match e {
        Effect::Allow {
            rule,
            entry,
            from: Origin::Listed,
            ..
        } => allow
            .get(rule)
            .is_some_and(|entries| entries.contains(entry)),
        Effect::Allow {
            from: Origin::Table,
            ..
        }
        | Effect::Withheld { .. }
        | Effect::Gate { .. }
        | Effect::Outcome { .. } => true,
    }
}

/// `e` as shown, held by `phrases`.
pub(super) fn effect(e: &Effect, phrases: &[String]) -> EffectView {
    let (kind, line, from) = match e {
        Effect::Gate { doc, .. } => (EffectKind::Gate, doc.name.clone(), None),
        Effect::Outcome { doc, .. } => (EffectKind::Outcome, doc.name.clone(), None),
        Effect::Allow { entry, from, .. } => {
            (EffectKind::Allow, entry.clone(), Some(from_view(*from)))
        }
        Effect::Withheld { request, from, .. } => (
            EffectKind::Withheld,
            request.to_string(),
            Some(from_view(*from)),
        ),
    };
    EffectView {
        id: e.id().to_string(),
        kind,
        line,
        key: e.key().to_string(),
        phrases: phrases.to_vec(),
        from,
    }
}

/// Where an allow or withheld effect came from, as shown.
const fn from_view(from: Origin) -> FromView {
    match from {
        Origin::Listed => FromView::Listed,
        Origin::Table => FromView::Table,
    }
}

/// The allow entries of `effects` by rule, in effect order.
pub(super) fn may(effects: &[&Effect]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in effects {
        if let Effect::Allow { rule, entry, .. } = e {
            out.entry(rule.clone()).or_default().push(entry.clone());
        }
    }
    out
}
