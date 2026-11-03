//! Setup layers: the clone, then each `.moor.yaml` setup line, each named by a key chained to
//! the layer below, so a changed line invalidates itself and every later layer.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// A line that runs longer than this, in milliseconds, is checkpointed after it ends.
pub const SLOW_LINE_MS: u64 = 20_000;

/// One layer's key: hex SHA-256 over the key below it and the layer's own text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayerKey(String);

impl LayerKey {
    /// A stored key, when it is 64 hex characters.
    #[must_use]
    pub fn restore(s: String) -> Option<Self> {
        (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(Self(s))
    }

    /// The key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key of `parts`, each length-prefixed so no text can shift between them.
    fn of(parts: &[&str]) -> Self {
        let mut hash = Sha256::new();
        for part in parts {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        Self(hash.finalize().iter().fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        }))
    }
}

/// The key of every layer: the first is the clone of `repo` at commit `base`, then one per line.
#[must_use]
pub fn chain(repo: &str, base: &str, lines: &[String]) -> Vec<LayerKey> {
    let mut keys = vec![LayerKey::of(&["clone", repo, base])];
    for line in lines {
        let below = keys.last().map_or("", LayerKey::as_str);
        let key = LayerKey::of(&["line", below, line]);
        keys.push(key);
    }
    keys
}

/// Whether layer `layer` of a setup of `lines` lines is checkpointed once it passes in `ms`.
///
/// A checkpoint holds memory and disk, so only the clone, a slow line and the last line earn one.
#[must_use]
pub const fn checkpoint_after(layer: usize, ms: u64, lines: usize) -> bool {
    layer == 0 || layer == lines || ms > SLOW_LINE_MS
}

/// The deepest layer at or below `failed` whose key is `stored`; none when no layer is.
#[must_use]
pub fn nearest(
    keys: &[LayerKey],
    failed: usize,
    stored: impl Fn(&LayerKey) -> bool,
) -> Option<usize> {
    keys.iter()
        .enumerate()
        .take(failed.saturating_add(1))
        .rev()
        .find(|(_, k)| stored(k))
        .map(|(layer, _)| layer)
}

/// The layer of the failed setup line `line` among `lines`; the top layer when none of them failed.
#[must_use]
pub fn failed_layer(lines: &[String], line: &str) -> usize {
    lines
        .iter()
        .position(|l| l == line)
        .map_or(lines.len(), |i| i + 1)
}

#[cfg(test)]
mod tests;
