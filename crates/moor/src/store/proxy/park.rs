//! Tunnel park requests: how an op asks a task's proxy supervisor to park or restore its tunnel.
//!
//! Both sides live in `meta` rows keyed per task, so no schema change is needed. A request carries
//! a sequence number the supervisor echoes in its ack, which lets the op ignore stale acks.

use moor_core::TaskId;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::store::{Store, StoreError};

/// The tunnel state an op asks the supervisor for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunnelWant {
    /// Kill the tunnel and stop reviving it.
    Parked,
    /// Bring the tunnel back on the same box port.
    Up,
}

/// A stored tunnel request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TunnelRequest {
    /// The wanted state.
    pub(crate) want: TunnelWant,
    /// Request number, counting up from 1 per task.
    pub(crate) seq: u64,
}

/// The supervisor's answer to a tunnel request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunnelAck {
    /// The tunnel is parked.
    Parked {
        /// Sequence number of the answered request.
        seq: u64,
    },
    /// The tunnel is up.
    Up {
        /// Sequence number of the answered request.
        seq: u64,
    },
    /// The request failed.
    Failed {
        /// Sequence number of the answered request.
        seq: u64,
        /// Why it failed.
        error: String,
    },
}

impl TunnelAck {
    /// Sequence number of the request this answers.
    pub(crate) const fn seq(&self) -> u64 {
        match self {
            Self::Parked { seq } | Self::Up { seq } | Self::Failed { seq, .. } => *seq,
        }
    }

    /// Stored form: `<kind> <seq>`, plus ` <error>` for a failure.
    fn encode(&self) -> String {
        match self {
            Self::Parked { seq } => format!("parked {seq}"),
            Self::Up { seq } => format!("up {seq}"),
            Self::Failed { seq, error } => format!("failed {seq} {error}"),
        }
    }

    /// Parse the stored form.
    fn decode(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(3, ' ');
        let kind = parts.next()?;
        let seq = parts.next()?.parse().ok()?;
        let error = parts.next();
        match (kind, error) {
            ("parked", None) => Some(Self::Parked { seq }),
            ("up", None) => Some(Self::Up { seq }),
            ("failed", Some(error)) => Some(Self::Failed {
                seq,
                error: error.to_owned(),
            }),
            _ => None,
        }
    }
}

/// Key of the request row of `task`.
fn want_key(task: TaskId) -> String {
    format!("tunnel_want:{}", task.get())
}

/// Key of the ack row of `task`.
fn ack_key(task: TaskId) -> String {
    format!("tunnel_ack:{}", task.get())
}

/// Stored form of a request.
fn encode_request(want: TunnelWant, seq: u64) -> String {
    let word = match want {
        TunnelWant::Parked => "parked",
        TunnelWant::Up => "up",
    };
    format!("{word} {seq}")
}

/// Parse the stored form of a request.
fn decode_request(raw: &str) -> Option<TunnelRequest> {
    let (word, seq) = raw.split_once(' ')?;
    let want = match word {
        "parked" => TunnelWant::Parked,
        "up" => TunnelWant::Up,
        _ => return None,
    };
    Some(TunnelRequest {
        want,
        seq: seq.parse().ok()?,
    })
}

/// Read one `meta` value.
fn get(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?", [key], |r| r.get(0))
        .optional()?)
}

/// Write one `meta` value.
fn put(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
        params![key, value],
    )?;
    Ok(())
}

/// Delete the request and ack rows of `task` on `conn`.
pub(super) fn clear(conn: &Connection, task: TaskId) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM meta WHERE k IN (?, ?)",
        params![want_key(task), ack_key(task)],
    )?;
    Ok(())
}

impl Store {
    /// Records the wanted tunnel state; the sequence number is the previous request's plus one, or
    /// 1 when there is none. Returns it.
    pub(crate) fn request_tunnel(&self, task: TaskId, want: TunnelWant) -> Result<u64, StoreError> {
        let key = want_key(task);
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prev = match get(&tx, &key)? {
            Some(raw) => {
                decode_request(&raw)
                    .ok_or_else(|| StoreError::Corrupt(format!("tunnel request `{raw}`")))?
                    .seq
            }
            None => 0,
        };
        let seq = prev
            .checked_add(1)
            .ok_or_else(|| StoreError::Corrupt("tunnel request seq overflow".into()))?;
        put(&tx, &key, &encode_request(want, seq))?;
        tx.commit()?;
        drop(conn);
        Ok(seq)
    }

    /// The latest tunnel request of `task`, if any.
    pub(crate) fn tunnel_request(&self, task: TaskId) -> Result<Option<TunnelRequest>, StoreError> {
        let conn = self.reader()?;
        get(&conn, &want_key(task))?
            .map(|raw| {
                decode_request(&raw)
                    .ok_or_else(|| StoreError::Corrupt(format!("tunnel request `{raw}`")))
            })
            .transpose()
    }

    /// Records the supervisor's answer to a tunnel request of `task`.
    pub(crate) fn ack_tunnel(&self, task: TaskId, ack: &TunnelAck) -> Result<(), StoreError> {
        let conn = self.writer()?;
        put(&conn, &ack_key(task), &ack.encode())
    }

    /// The latest tunnel ack of `task`, if any.
    pub(crate) fn tunnel_ack(&self, task: TaskId) -> Result<Option<TunnelAck>, StoreError> {
        let conn = self.reader()?;
        get(&conn, &ack_key(task))?
            .map(|raw| {
                TunnelAck::decode(&raw)
                    .ok_or_else(|| StoreError::Corrupt(format!("tunnel ack `{raw}`")))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests;
