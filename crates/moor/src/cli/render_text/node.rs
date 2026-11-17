//! Text for `node ls`.

use std::fmt::Write as _;

use serde_json::Value;

use moor_core::decide::schedule::gib;
use moor_core::view::node::NodeSettingsValue;

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

/// Boxes holding a slot over the cap, `(no cap)` when there is none.
fn load_text(n: &Value) -> Option<String> {
    let used = n.get("used")?.as_u64()?;
    let used = n
        .get("max_boxes")
        .and_then(Value::as_u64)
        .map_or_else(|| format!("{used} (no cap)"), |m| format!("{used}/{m}"));
    let mem = match (
        n.get("mem_free_mib").and_then(Value::as_u64),
        n.get("mem_report_age_secs").and_then(Value::as_i64),
    ) {
        (Some(free), Some(age)) => {
            let psi = n
                .get("mem_psi_centi")
                .and_then(Value::as_u64)
                .map_or_else(String::new, |p| format!(" psi {}.{:02}", p / 100, p % 100));
            format!("  {} free{psi} {age}s ago", gib(free))
        }
        _ => String::new(),
    };
    let low = if n.get("disk_low").and_then(Value::as_bool) == Some(true) {
        " (low)"
    } else {
        ""
    };
    let disk = n
        .get("disk_free_mib")
        .and_then(Value::as_u64)
        .map_or_else(String::new, |d| format!("  {} disk free{low}", gib(d)));
    let held = n
        .get("box_memory_mib")
        .and_then(Value::as_u64)
        .map_or_else(String::new, |m| format!("  Boxes hold {}", gib(m)));
    let pressure = n
        .get("pressure_since")
        .and_then(Value::as_i64)
        .map_or_else(String::new, |t| {
            format!("  pressure: high since {}", stamp::utc(t))
        });
    Some(format!("{used}{mem}{disk}{held}{pressure}"))
}

/// What `node set` left: the node's cap and labels.
pub(super) fn settings_text(value: &Value) -> Option<String> {
    let v = serde_json::from_value::<NodeSettingsValue>(value.clone()).ok()?;
    let cap = v
        .max_boxes
        .map_or_else(|| "no cap".to_owned(), |m| format!("at most {m} Boxes"));
    let labels = if v.labels.is_empty() {
        "no labels".to_owned()
    } else {
        format!("labels {}", v.labels.join(","))
    };
    Some(format!(
        "{}: {cap}, {labels} (revision {})\n",
        v.name, v.revision
    ))
}

/// What `node add` enrolled.
pub(super) fn added_text(value: &Value) -> Option<String> {
    let name = value.get("name")?.as_str()?;
    let id = value.get("machine_id")?.as_i64()?;
    Some(format!("shared node \"{name}\" is machine {id}\n"))
}

/// One line per node: name, scope, what its Boxes run on, link state, used/capacity and labels.
/// That is how the agent says it provides Boxes, such as `docker`, else the node's runtime.
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
            text("provides").or_else(|| text("runtime"))?,
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
