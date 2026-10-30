//! Text for `user machine ls` and `user machine status`.

use std::fmt::Write as _;

use serde_json::Value;

use crate::stamp;

/// One line per machine: its name, whether its agent is linked, and the agent's version.
pub(super) fn list_text(value: &Value) -> Option<String> {
    let machines = value.get("machines")?.as_array()?;
    if machines.is_empty() {
        return Some("no machines\n".to_owned());
    }
    let mut out = String::new();
    for m in machines {
        let name = m.get("name")?.as_str()?;
        let link = m.get("link").filter(|l| !l.is_null());
        let state = match link.and_then(|l| l.get("online")?.as_bool()) {
            Some(true) => "online",
            Some(false) => "offline",
            None => "not linked",
        };
        let version = link.and_then(|l| l.get("version")?.as_str()).unwrap_or("-");
        let _ = writeln!(out, "{name}  {state}  {version}");
    }
    Some(out)
}

/// One line per field of an agent's status, then its recent ops, newest first.
pub(super) fn status_text(value: &Value) -> Option<String> {
    let text = |key: &str| value.get(key)?.as_str();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "version: {} (protocol {})",
        text("version")?,
        value.get("protocol")?.as_u64()?
    );
    let _ = writeln!(out, "os:      {} {}", text("os")?, text("arch")?);
    match value.get("free_disk_gb").and_then(Value::as_u64) {
        Some(gb) => writeln!(out, "disk:    {gb} GB free"),
        None => writeln!(out, "disk:    unknown"),
    }
    .ok()?;
    if let Some(pool) = value.get("pool").filter(|p| !p.is_null()) {
        pool_text(pool, &mut out)?;
    }
    let recent = value.get("recent")?.as_array()?;
    let _ = writeln!(out, "recent:  {} ops", recent.len());
    for op in recent.iter().rev() {
        op_line(op, &mut out)?;
    }
    Some(out)
}

/// The provider, crabbox version and leases of a pool host.
fn pool_text(pool: &Value, out: &mut String) -> Option<()> {
    let _ = writeln!(out, "pool:    {}", pool.get("provider")?.as_str()?);
    let _ = writeln!(out, "crabbox: {}", pool.get("crabbox")?.as_str()?);
    if let Some(e) = pool.get("error").and_then(Value::as_str) {
        let _ = writeln!(out, "leases:  not listed: {e}");
    }
    for lease in pool.get("leases")?.as_array()? {
        let slug = lease.get("slug")?.as_str()?;
        let _ = writeln!(out, "lease:   {slug} {}", lease.get("state")?.as_str()?);
    }
    Some(())
}

/// `time op target ok|error`, indented under the recent heading.
fn op_line(op: &Value, out: &mut String) -> Option<()> {
    let at = stamp::utc(op.get("at")?.as_i64()?);
    let name = op.get("op")?.as_str()?;
    let target = op.get("target").and_then(Value::as_str).unwrap_or("-");
    let _ = write!(out, "  {at} {name} {target} ");
    match op.get("error").and_then(Value::as_str) {
        Some(e) => writeln!(out, "error: {e}"),
        None => writeln!(out, "ok"),
    }
    .ok()
}
