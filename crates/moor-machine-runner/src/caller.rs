//! Who is calling: the headers the Box's Proxy sets on every request.

use http::HeaderMap;
use moor_core::box_key::BoxKey;
use moor_core::config::check_exclude;
use moor_core::BoxId;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

/// Header carrying the calling Box's numeric store id.
const BOX: &str = "X-Moor-Box";
/// Header carrying the attached machine's name.
const MACHINE: &str = "X-Moor-Machine";
/// Header carrying the Box's slot on the machine, which sets its ports.
const SLOT: &str = "X-Moor-Machine-Slot";
/// Header carrying what syncs leave alone, as a JSON array of rsync patterns; optional.
const EXCLUDE: &str = "X-Moor-Machine-Exclude";
/// Header carrying the machine's enrolled id, under which its agent links in.
const MACHINE_ID: &str = "X-Moor-Machine-Id";
/// Header carrying why the Box holds no machine slot; set instead of the machine headers.
const MISSING: &str = "X-Moor-Machine-Missing";
/// Highest slot: its ports, the repo's plus ten per slot, must stay below 65536.
const MAX_SLOT: u16 = 6553;

/// The Box a request comes from and the machine attached to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// The calling Box.
    pub(crate) box_id: BoxId,
    /// The machine's name, for messages.
    pub(crate) machine: String,
    /// How the runner reaches the machine.
    pub(crate) reach: Reach,
    /// The Box's slot on the machine: 0 up to [`MAX_SLOT`].
    pub(crate) slot: u16,
    /// rsync patterns every sync leaves alone, from the Box's config.
    pub(crate) sync_exclude: Vec<String>,
}

impl Caller {
    /// Reads the caller from the request headers, naming the first one missing or malformed.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Result<Self, ErrorData> {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| refused(format!("missing header {name}")))
        };
        let box_id = box_id(headers)?;
        let machine = header(MACHINE)?;
        let machine_id =
            machine_id(headers)?.ok_or_else(|| refused(format!("missing header {MACHINE_ID}")))?;
        let reach = Reach {
            machine_id,
            machine: machine.clone(),
        };
        let raw = header(SLOT)?;
        let slot = raw
            .parse::<u16>()
            .ok()
            .filter(|n| *n <= MAX_SLOT)
            .ok_or_else(|| {
                refused(format!(
                    "header {SLOT} is not a slot 0 to {MAX_SLOT}: {raw}"
                ))
            })?;
        let sync_exclude = match headers.get(EXCLUDE) {
            None => Vec::new(),
            Some(raw) => raw
                .to_str()
                .ok()
                .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
                .ok_or_else(|| refused(format!("header {EXCLUDE} is not a JSON array of strings")))
                .and_then(|list| {
                    list.iter()
                        .try_for_each(|p| check_exclude(p))
                        .map_err(|e| refused(format!("header {EXCLUDE}: {e}")))
                        .map(|()| list)
                })?,
        };
        Ok(Self {
            box_id,
            machine,
            reach,
            slot,
            sync_exclude,
        })
    }

    /// Why the Box holds no machine slot, when the Proxy says so; `None` for a Box that holds one.
    ///
    /// # Errors
    /// Returns an MCP error when the marker comes without a valid Box id.
    pub(crate) fn missing(headers: &HeaderMap) -> Result<Option<String>, ErrorData> {
        let Some(reason) = headers.get(MISSING) else {
            return Ok(None);
        };
        box_id(headers)?;
        let reason = reason
            .to_str()
            .map_err(|_| refused(format!("header {MISSING} is not text")))?;
        Ok(Some(reason.trim().to_owned()))
    }

    /// The calling Box's id, which every request carries, with a machine or without.
    pub(crate) fn box_of(headers: &HeaderMap) -> Result<BoxId, ErrorData> {
        box_id(headers)
    }

    /// The Box's copy on the machine, relative to the machine user's home.
    pub(crate) fn remote_dir(&self) -> String {
        BoxKey::new(self.box_id).machine_dir()
    }
}

/// How the runner reaches a machine: only over its agent's link, under its enrolled id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reach {
    /// The enrolled id its agent links in under.
    pub(crate) machine_id: i64,
    /// Its name, for errors.
    pub(crate) machine: String,
}

/// The machine's enrolled id from the request headers, when its agent links in.
fn machine_id(headers: &HeaderMap) -> Result<Option<i64>, ErrorData> {
    let Some(raw) = headers.get(MACHINE_ID) else {
        return Ok(None);
    };
    raw.to_str()
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .map(Some)
        .ok_or_else(|| refused(format!("header {MACHINE_ID} is not a machine id")))
}

/// The calling Box's id from the request headers.
fn box_id(headers: &HeaderMap) -> Result<BoxId, ErrorData> {
    let raw = headers
        .get(BOX)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| refused(format!("missing header {BOX}")))?;
    raw.parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .map(BoxId::new)
        .ok_or_else(|| refused(format!("header {BOX} is not a Box id: {raw}")))
}

/// An MCP error for a request the runner will not serve.
fn refused(message: String) -> ErrorData {
    ErrorData::invalid_request(message, None)
}
