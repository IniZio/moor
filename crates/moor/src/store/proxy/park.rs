//! Tunnel park requests: how an op asks a box's proxy supervisor to park or restore its tunnel.
//!
//! Both sides live in `meta` rows keyed per box, so no schema change is needed. A request carries
//! a sequence number the supervisor echoes in its ack, which lets the op ignore stale acks.

use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::store::{Store, StoreError};

/// The tunnel state an op asks the supervisor for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunnelWant {
    /// Kill the tunnel and stop reviving it.
    Parked,
    /// Bring the tunnel back on the same box port after the Box was stopped and booted again.
    Up,
    /// Bring the tunnel back on the same box port, the Box having stayed up throughout.
    UpAwake,
}

/// A stored tunnel request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TunnelRequest {
    /// The wanted state.
    pub(crate) want: TunnelWant,
    /// Request number, counting up from 1 per box.
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

/// Key of the request row of `box_id`.
fn want_key(box_id: BoxId) -> String {
    format!("tunnel_want:{}", box_id.get())
}

/// Key of the ack row of `box_id`.
fn ack_key(box_id: BoxId) -> String {
    format!("tunnel_ack:{}", box_id.get())
}

/// Key of the row recording that a supervisor has opened a tunnel into the Box of `box_id`.
fn opened_key(box_id: BoxId) -> String {
    format!("tunnel_opened:{}", box_id.get())
}

/// Stored form of a request.
fn encode_request(want: TunnelWant, seq: u64) -> String {
    let word = match want {
        TunnelWant::Parked => "parked",
        TunnelWant::Up => "up",
        TunnelWant::UpAwake => "up_awake",
    };
    format!("{word} {seq}")
}

/// Parse the stored form of a request.
fn decode_request(raw: &str) -> Option<TunnelRequest> {
    let (word, seq) = raw.split_once(' ')?;
    let want = match word {
        "parked" => TunnelWant::Parked,
        "up" => TunnelWant::Up,
        "up_awake" => TunnelWant::UpAwake,
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

/// Delete the request and ack rows of `box_id` on `conn`.
pub(super) fn clear(conn: &Connection, box_id: BoxId) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM meta WHERE k IN (?, ?, ?)",
        params![want_key(box_id), ack_key(box_id), opened_key(box_id)],
    )?;
    Ok(())
}

impl Store {
    /// Records the wanted tunnel state; the sequence number is the previous request's plus one, or
    /// 1 when there is none. Returns it.
    pub(crate) fn request_tunnel(
        &self,
        box_id: BoxId,
        want: TunnelWant,
    ) -> Result<u64, StoreError> {
        let key = want_key(box_id);
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

    /// The latest tunnel request of `box_id`, if any.
    pub(crate) fn tunnel_request(
        &self,
        box_id: BoxId,
    ) -> Result<Option<TunnelRequest>, StoreError> {
        let conn = self.reader()?;
        get(&conn, &want_key(box_id))?
            .map(|raw| {
                decode_request(&raw)
                    .ok_or_else(|| StoreError::Corrupt(format!("tunnel request `{raw}`")))
            })
            .transpose()
    }

    /// Records the supervisor's answer to a tunnel request of `box_id`.
    pub(crate) fn ack_tunnel(&self, box_id: BoxId, ack: &TunnelAck) -> Result<(), StoreError> {
        let conn = self.writer()?;
        put(&conn, &ack_key(box_id), &ack.encode())
    }

    /// Records that a tunnel is about to be opened into the Box of `box_id`; it stays set until the
    /// proxy row is removed.
    pub(crate) fn note_tunnel_opened(&self, box_id: BoxId) -> Result<(), StoreError> {
        let conn = self.writer()?;
        put(&conn, &opened_key(box_id), "1")
    }

    /// Whether a tunnel was ever opened into the Box of `box_id`.
    pub(crate) fn tunnel_opened(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let conn = self.reader()?;
        Ok(get(&conn, &opened_key(box_id))?.is_some())
    }

    /// The latest tunnel ack of `box_id`, if any.
    pub(crate) fn tunnel_ack(&self, box_id: BoxId) -> Result<Option<TunnelAck>, StoreError> {
        let conn = self.reader()?;
        get(&conn, &ack_key(box_id))?
            .map(|raw| {
                TunnelAck::decode(&raw)
                    .ok_or_else(|| StoreError::Corrupt(format!("tunnel ack `{raw}`")))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests;
