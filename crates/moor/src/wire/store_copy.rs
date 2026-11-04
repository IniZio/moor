//! `moor store copy`.

use crate::env::Env;

/// Copies the `SQLite` store in the state dir `env` names into the empty Postgres database at
/// `to`; the report to print, or why nothing was copied.
pub fn copy_store(env: &Env, to: &str) -> Result<String, String> {
    let state_dir = env
        .state_dir()
        .ok_or("no state directory: set MOOR_STATE_DIR, XDG_STATE_HOME or HOME")?;
    let target = crate::store::db::Target::parse(to).map_err(|e| format!("--to: {e}"))?;
    let source = state_dir.join(crate::store::STORE_FILE);
    let copied =
        crate::store::copy::copy_to_postgres(&source, &target).map_err(|e| e.to_string())?;
    let mut lines = vec![format!(
        "copied the store in {} to {}",
        source.display(),
        target.shown()
    )];
    lines.extend(copied.iter().map(|c| format!("{:>8}  {}", c.rows, c.table)));
    lines.push(format!(
        "{:>8}  rows in {} tables",
        copied.iter().map(|c| c.rows).sum::<i64>(),
        copied.len()
    ));
    Ok(lines.join("\n"))
}
