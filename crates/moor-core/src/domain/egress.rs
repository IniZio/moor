//! The hosts a Box reached: one row per host, port and path. The machine agent keeps a Docker
//! Box's rows in [`SEEN_FILE`], and the Box's keeper writes them into `egress_seen` events.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// The file in a Docker Box's agent place that holds its rows, as a JSON array.
pub const SEEN_FILE: &str = "/run/moor/egress.json";

/// Longest a changed count waits for a new row before an event carries it anyway, in ms.
pub const COUNT_EVERY_MS: u64 = 10 * 60 * 1000;

/// How a connection left the Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPath {
    /// tinyproxy dialled the host itself.
    Proxied,
    /// tinyproxy sent it to the Proxy, which adds a credential.
    Credential,
    /// It skipped tinyproxy.
    Direct,
}

/// One host a Box reached, on one port and path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRow {
    /// The name the client asked for; an IP address for [`EgressPath::Direct`].
    pub host: String,
    /// The destination port.
    pub port: u16,
    /// How the connection left.
    pub path: EgressPath,
    /// When it was first seen, in Unix milliseconds.
    pub first_seen: u64,
    /// Requests through tinyproxy, or new connections when direct.
    pub count: u64,
}

/// A row's identity.
type Key = (String, u16, EgressPath);

/// A Box's rows, so a host seen again only adds to its count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressTally(BTreeMap<Key, (u64, u64)>);

impl EgressTally {
    /// The tally of `rows`; rows with the same host, port and path merge.
    #[must_use]
    pub fn of(rows: Vec<EgressRow>) -> Self {
        let mut tally = Self::default();
        for r in rows {
            tally.add(&r.host, r.port, r.path, r.first_seen, r.count);
        }
        tally
    }

    /// Adds `count` sightings of `host` on `port` by `path` at `at`.
    pub fn add(&mut self, host: &str, port: u16, path: EgressPath, at: u64, count: u64) {
        let (first, n) = self
            .0
            .entry((host.to_owned(), port, path))
            .or_insert((at, 0));
        *first = (*first).min(at);
        *n = n.saturating_add(count);
    }

    /// Whether it holds the row of `host` on `port` by `path`.
    #[must_use]
    pub fn contains(&self, host: &str, port: u16, path: EgressPath) -> bool {
        self.0.contains_key(&(host.to_owned(), port, path))
    }

    /// The rows, ordered by host, port and path.
    #[must_use]
    pub fn rows(&self) -> Vec<EgressRow> {
        self.0
            .iter()
            .map(|((host, port, path), (first_seen, count))| EgressRow {
                host: host.clone(),
                port: *port,
                path: *path,
                first_seen: *first_seen,
                count: *count,
            })
            .collect()
    }
}

/// What a Box's `egress_seen` events already carried.
#[derive(Debug, Default)]
pub struct EgressFlushed {
    /// Each row's count as last written.
    counts: HashMap<Key, u64>,
    /// When the last event was written, in Unix milliseconds.
    last: Option<u64>,
}

impl EgressFlushed {
    /// The rows of `rows` the next event carries at `now`, marked as written: every row that is
    /// new or whose count changed. With no new row, changed counts wait until [`COUNT_EVERY_MS`]
    /// passed since the last event. Empty when there is nothing to write.
    pub fn take(&mut self, rows: &[EgressRow], now: u64) -> Vec<EgressRow> {
        let key = |r: &EgressRow| (r.host.clone(), r.port, r.path);
        let changed: Vec<&EgressRow> = rows
            .iter()
            .filter(|r| self.counts.get(&key(r)) != Some(&r.count))
            .collect();
        let any_new = changed.iter().any(|r| !self.counts.contains_key(&key(r)));
        let due = self
            .last
            .is_none_or(|last| now.saturating_sub(last) >= COUNT_EVERY_MS);
        if changed.is_empty() || !(any_new || due) {
            return Vec::new();
        }
        for r in &changed {
            self.counts.insert(key(r), r.count);
        }
        self.last = Some(now);
        changed.into_iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests;
