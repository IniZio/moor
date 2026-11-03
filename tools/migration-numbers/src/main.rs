//! `migration-numbers [<base ref>]`: fails when the store's migrations collide with the base's.
//!
//! The base is the argument, else `$BASE`, else `origin/develop`.

use std::path::Path;
use std::process::{Command, ExitCode};

use migration_numbers::{step_names, violations, FIX, STEPS_FILE};

fn run() -> Result<Vec<String>, String> {
    let base_ref = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("BASE").ok().filter(|b| !b.is_empty()))
        .unwrap_or_else(|| "origin/develop".to_owned());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let shown = Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("show")
        .arg(format!("{base_ref}:{STEPS_FILE}"))
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !shown.status.success() {
        return Err(format!(
            "cannot read {STEPS_FILE} on `{base_ref}`: {}\nfetch it (git fetch origin develop) or name \
             another base with BASE=<ref>",
            String::from_utf8_lossy(&shown.stderr).trim()
        ));
    }
    let base = step_names(&String::from_utf8_lossy(&shown.stdout))?;
    let head: Vec<&str> = moor_schema::STEPS.iter().map(|s| s.name).collect();
    let bad = violations(&base_ref, &base, &head)?;
    if bad.is_empty() {
        println!(
            "migration-numbers: {} steps ok against {base_ref}",
            head.len()
        );
    }
    Ok(bad)
}

fn main() -> ExitCode {
    match run() {
        Ok(bad) if bad.is_empty() => ExitCode::SUCCESS,
        Ok(bad) => {
            for line in bad {
                eprintln!("migration-numbers: {line}");
            }
            eprintln!("{FIX}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("migration-numbers: {e}");
            ExitCode::FAILURE
        }
    }
}
