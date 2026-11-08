//! Why a setup line failed, told from its run and what the Proxy did while it ran.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use moor_core::BoxId;

use crate::ops::common::setup::SetupRun;
use crate::store::Store;

/// Most characters of output the failure keeps.
const TAIL_CHARS: usize = 2000;

/// Most Proxy refusals the failure names.
const NAMED: usize = 5;

/// What to record beside a failed `run` of Box `id`; none when the run did not fail or has
/// nothing to add.
pub(super) fn setup_why(store: &Store, id: BoxId, run: &SetupRun) -> Option<String> {
    let SetupRun::Failed { exit, tail, .. } = run else {
        return None;
    };
    let mut why = String::new();
    if let Some(code) = exit {
        let _ = write!(why, "exit {code}");
    }
    let tail = last_chars(tail);
    if !tail.is_empty() {
        if !why.is_empty() {
            why.push('\n');
        }
        let _ = write!(why, "output ended:\n{tail}");
    }
    for refusal in refusals(store, id) {
        if !why.is_empty() {
            why.push('\n');
        }
        let _ = write!(why, "while it ran, the Proxy {refusal}");
    }
    Some(why).filter(|w| !w.is_empty())
}

/// `text` cut to its last [`TAIL_CHARS`] characters.
fn last_chars(text: &str) -> &str {
    let skip = text.chars().count().saturating_sub(TAIL_CHARS);
    text.char_indices()
        .nth(skip)
        .and_then(|(at, _)| text.get(at..))
        .unwrap_or(text)
}

/// What the Proxy withheld or refused since the failing line began, each told once.
fn refusals(store: &Store, id: BoxId) -> Vec<String> {
    let newest = |kind: &str, n: usize| store.newest_events_of(id, kind, n).unwrap_or_default();
    let began = newest("setup_line_started", 1)
        .pop()
        .or_else(|| newest("setting_up", 1).pop())
        .map_or(0, |e| e.id);
    let text = |p: &serde_json::Value, key: &str| {
        p.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let mut seen = BTreeSet::new();
    let mut told = Vec::new();
    let mut rows = newest("credential_withheld", 50);
    rows.extend(newest("egress_refused", 50));
    rows.sort_by_key(|e| e.id);
    for e in rows.iter().filter(|e| e.id > began) {
        let host = text(&e.payload, "host").unwrap_or_default();
        let rule = text(&e.payload, "rule").map_or_else(String::new, |r| format!(" (rule {r})"));
        let verb = if e.kind == "egress_refused" {
            "refused"
        } else {
            "withheld"
        };
        let line = format!("{verb} {host}{rule}");
        if seen.insert(line.clone()) && told.len() < NAMED {
            told.push(line);
        }
    }
    told
}
