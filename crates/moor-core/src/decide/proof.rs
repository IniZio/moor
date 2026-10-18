//! Proof files an agent leaves under the Box workdir: listing, parsing and copy decisions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::box_key::BoxKey;
use crate::name::BoxId;

pub use crate::domain::proof::ProofFile;

/// Directory, relative to the Box workdir, where agents write proof files.
pub const PROOF_DIR: &str = ".moor/proof";

/// POSIX sh that prints `name<TAB>size<TAB>sha256` for each regular file under the proof directory.
const SCRIPT: &str = r#"[ -L .moor ] && exit 0
[ -L .moor/proof ] && exit 0
[ -d .moor/proof ] || exit 0
cd .moor/proof || exit 0
find . -type f ! -path '*
*' | while IFS= read -r f; do
  n=${f#./}
  s=$(wc -c <"$f" | tr -d ' ')
  h=$(sha256sum <"$f" 2>/dev/null | cut -d' ' -f1)
  [ -n "$h" ] || h=$(shasum -a 256 <"$f" 2>/dev/null | cut -d' ' -f1)
  printf '%s\t%s\t%s\n' "$n" "$s" "$h"
done
"#;

/// Argv that lists proof files; run it with the Box workdir as cwd.
#[must_use]
pub fn listing_argv() -> Vec<String> {
    vec!["sh".to_owned(), "-c".to_owned(), SCRIPT.to_owned()]
}

/// Parse the output of [`listing_argv`], dropping malformed lines and unsafe names, sorted by name.
#[must_use]
pub fn parse_listing(stdout: &str) -> Vec<ProofFile> {
    let mut by_name: HashMap<String, ProofFile> = HashMap::new();
    for file in stdout.lines().filter_map(parse_line) {
        by_name.insert(file.name.clone(), file);
    }
    let mut files: Vec<ProofFile> = by_name.into_values().collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

/// Parse one listing line, or `None` when it is malformed or names an unsafe path.
fn parse_line(line: &str) -> Option<ProofFile> {
    let mut parts = line.split('\t');
    let name = parts.next()?;
    let size = parts.next()?.trim().parse::<u64>().ok()?;
    let sha256 = parts.next()?.trim();
    if parts.next().is_some() || !valid_sha256(sha256) || !safe_name(name) {
        return None;
    }
    Some(ProofFile {
        name: name.to_owned(),
        size,
        sha256: sha256.to_owned(),
    })
}

/// True for exactly 64 lowercase hex digits.
fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// True for a non-empty relative name with no empty, `.` or `..` segments.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Files in `listing` that are new or whose hash differs from the latest `seen` entry of that name.
#[must_use]
pub fn fresh(listing: &[ProofFile], seen: &[ProofFile]) -> Vec<ProofFile> {
    let latest: HashMap<&str, &str> = seen
        .iter()
        .map(|f| (f.name.as_str(), f.sha256.as_str()))
        .collect();
    let mut emitted: HashSet<&str> = HashSet::new();
    listing
        .iter()
        .filter(|f| latest.get(f.name.as_str()) != Some(&f.sha256.as_str()))
        .rev()
        .filter(|f| emitted.insert(f.name.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Where a copied proof file lives, relative to the state dir: keyed by the Box id, never its name.
#[must_use]
pub fn copy_relpath(id: BoxId, file: &str) -> PathBuf {
    BoxKey::new(id).proof_file(file)
}

/// Plain sentence explaining why `file` is not copied, or `None` when it fits under `cap` bytes.
#[must_use]
pub fn refusal(file: &ProofFile, cap: u64) -> Option<String> {
    if file.size <= cap {
        return None;
    }
    Some(format!(
        "proof file {} is {} bytes, above the {cap}-byte proof cap; not copied. \
         Raise proof_max_bytes in the controller config to keep it.",
        file.name, file.size
    ))
}

#[cfg(test)]
mod tests;
