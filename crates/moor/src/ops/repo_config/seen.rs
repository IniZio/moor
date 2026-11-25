//! What a repo's Boxes met under their Proxy rules: the requests a rule sent on without its
//! secret, and the hosts reached without the Proxy.

use std::collections::{BTreeMap, BTreeSet};

use moor_core::view::repo_config::{DialledSeen, RulesSeen, WithheldSeen};
use serde_json::Value;

/// How many rows each list keeps.
const ROWS: usize = 20;

/// The lists of `events`: a repo's `credential_withheld` and `egress_seen` events as (Box, kind,
/// payload), oldest first. A Box's `egress_seen` rows carry running counts, so its newest count
/// of a row stands.
pub(super) fn seen(events: &[(i64, String, Value)], days: u32) -> RulesSeen {
    let mut withheld: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
    let mut counts: BTreeMap<(i64, String, u16, String), u64> = BTreeMap::new();
    for (box_id, kind, payload) in events {
        match kind.as_str() {
            "credential_withheld" => {
                let request = payload
                    .get("shape")
                    .and_then(Value::as_str)
                    .map_or_else(|| text(payload, "method"), str::to_owned);
                withheld
                    .entry((request, text(payload, "host")))
                    .or_default()
                    .insert(*box_id);
            }
            "egress_seen" => {
                let rows = payload.get("hosts").and_then(Value::as_array);
                for row in rows.into_iter().flatten() {
                    let path = text(row, "path");
                    if path == "credential" {
                        continue;
                    }
                    let port = row.get("port").and_then(Value::as_u64);
                    let port = port.and_then(|p| u16::try_from(p).ok()).unwrap_or_default();
                    let count = row.get("count").and_then(Value::as_u64).unwrap_or_default();
                    counts.insert((*box_id, text(row, "host"), port, path), count);
                }
            }
            _ => {}
        }
    }
    let mut withheld: Vec<WithheldSeen> = withheld
        .into_iter()
        .map(|((request, host), boxes)| WithheldSeen {
            request,
            host,
            boxes: u32::try_from(boxes.len()).unwrap_or(u32::MAX),
        })
        .collect();
    withheld.sort_by_key(|w| std::cmp::Reverse(w.boxes));
    withheld.truncate(ROWS);
    let mut summed: BTreeMap<(String, u16), u64> = BTreeMap::new();
    for ((_, host, port, _), count) in counts {
        *summed.entry((host, port)).or_default() += count;
    }
    let mut dialled: Vec<DialledSeen> = summed
        .into_iter()
        .map(|((host, port), count)| DialledSeen { host, port, count })
        .collect();
    dialled.sort_by_key(|d| std::cmp::Reverse(d.count));
    dialled.truncate(ROWS);
    RulesSeen {
        days,
        withheld,
        dialled,
    }
}

/// The string at `key` of `value`, empty when it has none.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests;
