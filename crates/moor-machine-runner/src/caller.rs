//! Who is calling: the headers the Box's Proxy sets on every request.

use http::HeaderMap;
use moor_core::box_key::BoxKey;
use moor_core::config::MachineKind;
use moor_core::config::{check_exclude, check_ssh};
use moor_core::BoxId;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

use crate::route::Route;

/// Header carrying the calling Box's numeric store id.
const BOX: &str = "X-Moor-Box";
/// Header carrying the attached machine's name.
const MACHINE: &str = "X-Moor-Machine";
/// Header carrying the machine's ssh target, such as `user@host`.
const SSH: &str = "X-Moor-Machine-Ssh";
/// Header carrying the Box's slot on the machine, which sets its ports.
const SLOT: &str = "X-Moor-Machine-Slot";
/// Header carrying what syncs leave alone, as a JSON array of rsync patterns; optional.
const EXCLUDE: &str = "X-Moor-Machine-Exclude";
/// Header carrying the machine's enrolled id when its agent links in; the ssh one is then unused.
const MACHINE_ID: &str = "X-Moor-Machine-Id";
/// Header carrying `vm-host` when the Box gets a VM on the machine; optional, `physical` when absent.
const KIND: &str = "X-Moor-Machine-Kind";
/// Header carrying a VM host's crabbox provider.
const VM: &str = "X-Moor-Machine-Vm";
/// Header naming the Box ports the controller serves on the machine, moved by the slot.
const PORTS: &str = "X-Moor-Machine-Ports";
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
    /// The machine itself, or a VM host that leases the Box a VM.
    pub(crate) kind: MachineKind,
    /// Where commands land once ssh reaches the machine; into the VM once it is ready.
    pub(crate) route: Route,
    /// The Box ports served on the machine's loopback, moved by the slot.
    pub(crate) ports: Vec<u16>,
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
        let reach = if let Some(machine_id) = machine_id(headers)? {
            Reach::Linked {
                machine_id,
                machine: machine.clone(),
            }
        } else {
            let ssh = header(SSH)?;
            check_ssh(&ssh).map_err(|e| refused(format!("header {SSH}: {e}")))?;
            Reach::Ssh { ssh }
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
        let kind = kind(headers)?;
        let ports = ports(headers)?;
        Ok(Self {
            box_id,
            machine,
            reach,
            slot,
            sync_exclude,
            kind,
            route: Route::Host,
            ports,
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

    /// What to add to a repo's port: 10 times the slot, or none in a VM of the Box's own.
    pub(crate) fn port_offset(&self) -> u32 {
        match self.kind {
            MachineKind::Physical => 10 * u32::from(self.slot),
            MachineKind::VmHost { .. } => 0,
        }
    }

    /// Each served port with the repo's own port it carries in the Box's VM: `(vm, host)`.
    pub(crate) fn vm_ports(&self) -> Vec<(u16, u16)> {
        let offset = 10 * self.slot;
        self.ports
            .iter()
            .filter_map(|p| p.checked_sub(offset).map(|vm| (vm, *p)))
            .collect()
    }

    /// `vm-host` for a Box with a VM on the machine, which tool results name; none otherwise.
    pub(crate) fn vm_host(&self) -> Option<&'static str> {
        self.kind.provider().map(|_| self.kind.name().as_str())
    }

    /// The Box's copy on the machine, relative to the machine user's home.
    pub(crate) fn remote_dir(&self) -> String {
        BoxKey::new(self.box_id).machine_dir()
    }
}

/// How the runner reaches a machine: one with an enrolled id only over its agent's link, so no
/// ssh target is ever made up for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Reach {
    /// Its agent links in under this enrolled id.
    Linked {
        /// The enrolled id.
        machine_id: i64,
        /// Its name, for errors.
        machine: String,
    },
    /// The host dials this ssh target.
    Ssh {
        /// Such as `user@host`.
        ssh: String,
    },
}

/// The machine's kind from the request headers: physical unless they say `vm-host`.
fn kind(headers: &HeaderMap) -> Result<MachineKind, ErrorData> {
    match headers.get(KIND).map(|v| v.to_str().map(str::trim)) {
        None | Some(Ok("physical")) => Ok(MachineKind::Physical),
        Some(Ok("vm-host")) => {
            let provider = headers.get(VM).and_then(|v| v.to_str().ok()).map(str::trim);
            match provider.filter(|p| !p.is_empty()) {
                Some(p) => Ok(MachineKind::VmHost {
                    provider: p.to_owned(),
                }),
                None => Err(refused(format!("missing header {VM}"))),
            }
        }
        Some(other) => Err(refused(format!("header {KIND} is not a kind: {other:?}"))),
    }
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

/// The ports in the ports header; none when it is missing.
fn ports(headers: &HeaderMap) -> Result<Vec<u16>, ErrorData> {
    let Some(raw) = headers.get(PORTS).and_then(|v| v.to_str().ok()) else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u16>().ok().filter(|p| *p > 0))
        .collect::<Option<Vec<u16>>>()
        .ok_or_else(|| refused(format!("header {PORTS} is not a list of ports: {raw}")))
}
