//! What a Box holds of the machine service, and the `X-Moor-*` headers that carry it on a
//! plain request to the service. Only the shared egress Proxy sets them; the Box's own are
//! dropped. A Box that holds no slot is passed on too, marked with why, so its agent sees a tool
//! error and not an auth failure.

use moor_core::request::RunnerAddr;
use moor_core::BoxId;

use crate::proxy::ledger::{BoxLedger, LedgerError};

/// Prefix of the headers the Proxy owns; the Box's own are dropped.
const OWN_PREFIX: &str = "x-moor-";

/// What a Box holds of the machine service.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Hold {
    /// A slot on a machine.
    Held(Held),
    /// No slot. A client treats a 401 or 403 as a call for OAuth and drops the server's tools
    /// for good, so the service is told instead.
    Missing {
        /// The Box.
        box_id: BoxId,
        /// Why, as one line the agent reads.
        reason: String,
    },
}

/// The machine a Box holds, as the service learns it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Held {
    /// The Box.
    pub(crate) box_id: BoxId,
    /// The machine's name.
    pub(crate) name: String,
    /// Its enrolled id, which its agent links in to the service under.
    pub(crate) machine_id: i64,
    /// The Box's port slot on the machine.
    pub(crate) slot: u16,
    /// rsync patterns a sync to the machine leaves alone.
    pub(crate) sync_exclude: Vec<String>,
    /// The runner holding the machine's link: up, not draining and heard from lately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runner: Option<RunnerAddr>,
}

/// What the Box of `ledger` holds, or why it could not be read.
pub(crate) fn hold_or_missing(ledger: &BoxLedger) -> Hold {
    ledger.hold().unwrap_or_else(|e| Hold::Missing {
        box_id: ledger.box_id(),
        reason: match e {
            LedgerError::Refused(why) => why,
            e => format!("the Box's machine could not be read: {e}; try again later"),
        },
    })
}

/// `request` without any `X-Moor-*` header, with the front's own added.
pub(super) fn marked(request: &[u8], slot: &Hold) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(request)
        .ok()?
        .strip_suffix("\r\n\r\n")?;
    let own = match slot {
        Hold::Held(held) => vec![
            ("X-Moor-Box", held.box_id.get().to_string()),
            ("X-Moor-Machine", held.name.clone()),
            ("X-Moor-Machine-Id", held.machine_id.to_string()),
            ("X-Moor-Machine-Slot", held.slot.to_string()),
            (
                "X-Moor-Machine-Exclude",
                serde_json::to_string(&held.sync_exclude).ok()?,
            ),
        ],
        Hold::Missing { box_id, reason } => vec![
            ("X-Moor-Box", box_id.get().to_string()),
            ("X-Moor-Machine-Missing", reason.clone()),
        ],
    };
    if own
        .iter()
        .any(|(_, v)| v.bytes().any(|b| b.is_ascii_control()))
    {
        return None;
    }
    let theirs = |line: &&str| {
        line.get(..OWN_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(OWN_PREFIX))
    };
    let mut out = String::new();
    for line in text.split("\r\n").filter(|l| !theirs(l)) {
        out.push_str(line);
        out.push_str("\r\n");
    }
    for (name, value) in own {
        for part in [name, ": ", &value, "\r\n"] {
            out.push_str(part);
        }
    }
    out.push_str("\r\n");
    Some(out.into_bytes())
}
