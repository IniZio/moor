//! Setup layers: the clone, then each `.moor.yaml` setup line, each named by a key chained to
//! the layer below, so a changed line invalidates itself and every later layer.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::user::UserId;

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

/// What a chain of layers belongs to. A checkpoint is reused only under the same identity.
#[derive(Debug, Clone, Copy)]
pub struct LayerIdentity<'a> {
    /// Whose secrets and repo access the checkpoint may hold.
    pub user: UserId,
    /// The repo, `OWNER/NAME`.
    pub repo: &'a str,
    /// The cache plan, since a checkpoint holds its cache disks.
    pub cache: &'a str,
    /// The machine class, since a fork refuses another size.
    pub class: &'a str,
    /// The base commit, only for a repo that opted out of reuse across commits.
    pub pinned_base: Option<&'a str>,
}

/// The key of every layer of `identity`: the clone, then one per line.
///
/// The key holds no commit unless the repo pinned one, so a Box at a new commit finds the
/// checkpoint. What keeps that safe is the rerun of every line over a checkpoint of another base.
#[must_use]
pub fn chain(identity: &LayerIdentity<'_>, lines: &[String]) -> Vec<LayerKey> {
    let user = identity.user.get().to_string();
    let mut clone = vec![
        "clone",
        &user,
        identity.repo,
        identity.cache,
        identity.class,
    ];
    clone.extend(identity.pinned_base);
    let mut keys = vec![LayerKey::of(&clone)];
    for line in lines {
        let below = keys.last().map_or("", LayerKey::as_str);
        let key = LayerKey::of(&["line", below, line]);
        keys.push(key);
    }
    keys
}

/// What the layer above the last setup line holds, each part as the text that names it. Those
/// steps read no repo file, so its key holds no commit but what the top line's key does.
#[derive(Debug, Clone, Copy)]
pub struct FinishedSteps<'a> {
    /// The script that installs the agent.
    pub install: &'a str,
    /// The profile's commit.
    pub profile_sha: &'a str,
    /// The script that installs what the profile lists.
    pub plugins: &'a str,
    /// The commit of each repo that script reads.
    pub sources: &'a str,
    /// The environment those scripts run with.
    pub env: &'a str,
    /// The dotfiles.
    pub dotfiles: &'a str,
}

/// The key of the layer above `top`, the last setup line's.
#[must_use]
pub fn finished_key(top: &LayerKey, steps: &FinishedSteps<'_>) -> LayerKey {
    LayerKey::of(&[
        "finished",
        top.as_str(),
        steps.install,
        steps.profile_sha,
        steps.plugins,
        steps.sources,
        steps.env,
        steps.dotfiles,
    ])
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

/// The most ignored paths a Box's brief and start reply name.
pub const IGNORED_NAMED: usize = 20;

/// The disk under a Box that reran its setup lines over a checkpoint of another commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmDisk {
    /// The commit the checkpoint's disk holds.
    pub commit: String,
    /// How many tracked paths differ from that commit; none when git could not tell.
    pub changed: Option<u64>,
    /// The ignored paths that came with the disk.
    pub ignored: Vec<String>,
}

impl WarmDisk {
    /// What the Box's agent and whoever started it are told.
    #[must_use]
    pub fn note(&self) -> String {
        let commit = self.commit.get(..12).unwrap_or(&self.commit);
        let changed = self.changed.map_or_else(
            || "An unknown number of tracked paths".to_owned(),
            |n| format!("{n} tracked path{}", if n == 1 { "" } else { "s" }),
        );
        let mut note = format!(
            "This Box started from a disk set up at commit {commit}, and every setup line ran \
             again on the current tree. {changed} changed since that commit."
        );
        if self.ignored.is_empty() {
            return note;
        }
        let shown = self.ignored.iter().take(IGNORED_NAMED);
        let names = shown.map(String::as_str).collect::<Vec<_>>().join(", ");
        let more = if self.ignored.len() > IGNORED_NAMED {
            ", and more"
        } else {
            ""
        };
        let _ = write!(
            note,
            " Ignored paths from that disk remain: {names}{more}. A setup line may have skipped \
             rebuilding them, so they may be stale."
        );
        note
    }
}

#[cfg(test)]
mod tests;
