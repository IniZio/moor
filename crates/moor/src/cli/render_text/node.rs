//! Text for `node ls`.

use std::fmt::Write as _;

use serde_json::Value;

use crate::stamp;

/// `online`, or `offline since` when the node went down.
fn link_text(n: &Value) -> Option<String> {
    if n.get("online")?.as_bool()? {
        return Some("online".to_owned());
    }
    Some(format!(
        "offline since {}",
        stamp::utc(n.get("offline_since")?.as_i64()?)
    ))
}

/// Boxes holding a slot over the cap, `-` when there is none.
fn load_text(n: &Value) -> Option<String> {
    let used = n.get("used")?.as_u64()?;
    let cap = n
        .get("max_boxes")
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |m| m.to_string());
    Some(format!("{used}/{cap}"))
}

/// One line per node: name, scope, runtime, link state, used/capacity and labels.
pub(super) fn list_text(value: &Value) -> Option<String> {
    let nodes = value.get("nodes")?.as_array()?;
    if nodes.is_empty() {
        return Some("no nodes\n".to_owned());
    }
    let mut out = String::new();
    for n in nodes {
        let text = |key: &str| n.get(key)?.as_str();
        let _ = write!(
            out,
            "{}  {}  {}  {}  {}",
            text("name")?,
            text("scope")?,
            text("runtime")?,
            link_text(n)?,
            load_text(n)?
        );
        let waiting = n.get("waiting")?.as_u64()?;
        if waiting > 0 {
            let _ = write!(out, "  {waiting} waiting");
        }
        let labels: Vec<&str> = n
            .get("labels")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if !labels.is_empty() {
            let _ = write!(out, "  {}", labels.join(","));
        }
        out.push('\n');
    }
    Some(out)
}
