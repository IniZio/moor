//! Build script: embeds the git commit as `MOOR_COMMIT`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs git in the crate directory and returns trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_owned())
}

/// True when `s` is exactly 40 hex digits.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Picks the commit: env override, else git HEAD, else `unknown`.
fn commit() -> String {
    std::env::var("MOOR_COMMIT")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_owned())
        .or_else(|| git(&["rev-parse", "HEAD"]).filter(|s| is_full_sha(s)))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Lists the git paths whose change moves HEAD, resolved against `base`.
///
/// Cargo treats a missing `rerun-if-changed` path as always changed, so each path must exist.
/// A shallow or fresh clone has no `packed-refs`. A branch whose ref is only in `packed-refs`
/// has no loose ref file, so the nearest existing directory above it stands in: a new commit
/// writes the loose ref there.
fn watched_paths(base: &Path) -> Vec<PathBuf> {
    let git_path = |name: &str| git(&["rev-parse", "--git-path", name]).map(|p| base.join(p));
    let mut paths: Vec<PathBuf> = ["HEAD", "packed-refs"]
        .iter()
        .filter_map(|name| git_path(name))
        .filter(|p| p.exists())
        .collect();
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    if let Some(mut p) = head_ref.and_then(|r| git_path(&r)) {
        while !p.exists() && p.pop() {}
        paths.push(p);
    }
    paths
}

/// Writes the cargo directives to stdout.
fn main() {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "cargo:rerun-if-changed=build.rs");
    let _ = writeln!(out, "cargo:rerun-if-env-changed=MOOR_COMMIT");
    let base = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    for p in watched_paths(Path::new(&base)) {
        let _ = writeln!(out, "cargo:rerun-if-changed={}", p.display());
    }
    let _ = writeln!(out, "cargo:rustc-env=MOOR_COMMIT={}", commit());
}
