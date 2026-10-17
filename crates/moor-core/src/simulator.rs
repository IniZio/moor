//! Picking the iOS simulator a Task gets, and the commands that create it on the Mac.
//!
//! Everything here is pure: it parses what `xcrun simctl list` printed, chooses a
//! device type and runtime, and renders the shell text. Running it is the caller's job.

use serde::Deserialize;

use crate::boxscript::quote;
use crate::name::BoxPath;
use crate::task_key::TaskKey;

/// Shell text that prints the Mac's simulator device types and runtimes as JSON.
pub const LIST_SCRIPT: &str = "xcrun simctl list -j devicetypes runtimes";

/// A device type as listed under `devicetypes`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct DeviceType {
    /// Reverse-DNS identifier `simctl create` takes.
    identifier: String,
    /// Display name, such as `iPhone 17`.
    name: String,
    /// `iPhone`, `iPad` and so on; absent on some entries.
    #[serde(rename = "productFamily", default)]
    family: Option<String>,
    /// Oldest runtime this device type shipped with, as a number; absent counts as 0.
    #[serde(rename = "minRuntimeVersion", default)]
    min_runtime: u64,
}

/// A device type a runtime lists as supported.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Supported {
    /// Reverse-DNS identifier of the device type.
    identifier: String,
    /// Display name of the device type.
    #[serde(default)]
    name: String,
    /// Product family, when the runtime states it.
    #[serde(rename = "productFamily", default)]
    family: Option<String>,
}

/// A simulator runtime as listed under `runtimes`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Runtime {
    /// Reverse-DNS identifier `simctl create` takes.
    identifier: String,
    /// Display name, such as `iOS 26.4`.
    name: String,
    /// Dotted version, such as `26.4.1`.
    #[serde(default)]
    version: String,
    /// Whether the runtime can be used; absent counts as usable.
    #[serde(rename = "isAvailable", default = "yes")]
    available: bool,
    /// Platform name; absent is read from the identifier.
    #[serde(default)]
    platform: Option<String>,
    /// Device types this runtime can host.
    #[serde(rename = "supportedDeviceTypes", default)]
    supported: Vec<Supported>,
}

/// Serde default for a flag that is true when missing.
const fn yes() -> bool {
    true
}

impl Runtime {
    /// Reports whether this is an iOS runtime.
    fn is_ios(&self) -> bool {
        self.platform.as_ref().map_or_else(
            || self.identifier.contains("SimRuntime.iOS"),
            |p| p == "iOS",
        )
    }
}

/// What `xcrun simctl list -j devicetypes runtimes` reported.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Inventory {
    /// Every device type, in the order `simctl` printed them.
    #[serde(default)]
    devicetypes: Vec<DeviceType>,
    /// Every runtime, in the order `simctl` printed them.
    #[serde(default)]
    runtimes: Vec<Runtime>,
}

/// Parses the JSON that [`LIST_SCRIPT`] prints, ignoring fields it does not use.
///
/// # Errors
/// Returns the JSON error text when `json` is not a simulator inventory.
pub fn parse_inventory(json: &str) -> Result<Inventory, String> {
    serde_json::from_str(json).map_err(|e| format!("cannot read the simulator inventory: {e}"))
}

/// The device type and runtime a new simulator is created with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Identifier of the device type.
    pub device_type: String,
    /// Identifier of the runtime.
    pub runtime: String,
}

/// Why a pinned (or default) simulator choice cannot be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    /// The runtime is unknown, unavailable or missing; carries the reason.
    Runtime(String),
    /// The device type is unknown or unsupported by the runtime; carries the reason.
    DeviceType(String),
}

impl PinError {
    /// The config key the error is about.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Runtime(_) => "box.simulator.runtime",
            Self::DeviceType(_) => "box.simulator.device_type",
        }
    }

    /// The human reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::Runtime(r) | Self::DeviceType(r) => r,
        }
    }
}

/// Splits a dotted version into numbers; a part that is not a number counts as 0.
fn version_parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Compares dotted versions numerically per component, so `26.10` > `26.9`.
fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (version_parts(a), version_parts(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a.cmp(&b)
}

/// Picks the runtime: the pinned one, else the newest available iOS runtime.
fn pick_runtime<'a>(inv: &'a Inventory, pin: Option<&str>) -> Result<&'a Runtime, PinError> {
    let ios: Vec<&Runtime> = inv
        .runtimes
        .iter()
        .filter(|r| r.is_ios() && r.available)
        .collect();
    if let Some(pin) = pin {
        return ios
            .iter()
            .find(|r| r.name == pin || r.identifier == pin)
            .copied()
            .ok_or_else(|| {
                let names: Vec<&str> = ios.iter().map(|r| r.name.as_str()).collect();
                PinError::Runtime(format!(
                    "unknown simulator runtime \"{pin}\"; this Mac has: {}",
                    names.join(", ")
                ))
            });
    }
    let mut best: Option<&Runtime> = None;
    for r in ios {
        if best.is_none_or(|b| cmp_version(&r.version, &b.version).is_gt()) {
            best = Some(r);
        }
    }
    best.ok_or_else(|| PinError::Runtime("this Mac has no available iOS simulator runtime".into()))
}

/// Returns the product family of a supported device type, from the runtime or the global list.
fn family<'a>(inv: &'a Inventory, s: &'a Supported) -> Option<&'a str> {
    s.family.as_deref().or_else(|| {
        inv.devicetypes
            .iter()
            .find(|d| d.identifier == s.identifier)
            .and_then(|d| d.family.as_deref())
    })
}

/// Chooses the device type and runtime for a new simulator.
///
/// A pin matches by name or identifier. With no runtime pin the newest available iOS
/// runtime is used; with no device pin the newest iPhone type that runtime supports.
///
/// # Errors
/// Returns [`PinError::Runtime`] when the runtime is unknown or none is available, and
/// [`PinError::DeviceType`] when the device type is unknown, not an iPhone or iPad, or
/// not supported by the runtime.
pub fn choose(
    inv: &Inventory,
    device_type: Option<&str>,
    runtime: Option<&str>,
) -> Result<Choice, PinError> {
    let rt = pick_runtime(inv, runtime)?;
    let phone_or_pad = |s: &&Supported| matches!(family(inv, s), Some("iPhone" | "iPad"));
    let device = if let Some(pin) = device_type {
        rt.supported
            .iter()
            .filter(phone_or_pad)
            .find(|s| s.name == pin || s.identifier == pin)
            .ok_or_else(|| {
                let names: Vec<&str> = rt
                    .supported
                    .iter()
                    .filter(phone_or_pad)
                    .map(|s| s.name.as_str())
                    .collect();
                PinError::DeviceType(format!(
                    "unknown simulator device type \"{pin}\" for {}; it supports: {}",
                    rt.name,
                    names.join(", ")
                ))
            })?
    } else {
        let rank = |s: &Supported| {
            inv.devicetypes
                .iter()
                .enumerate()
                .find(|(_, d)| d.identifier == s.identifier)
                .map_or((0, usize::MAX), |(i, d)| (d.min_runtime, i))
        };
        let mut best: Option<&Supported> = None;
        for s in rt
            .supported
            .iter()
            .filter(|s| family(inv, s) == Some("iPhone"))
        {
            let better = best.is_none_or(|b| {
                let ((m, i), (bm, bi)) = (rank(s), rank(b));
                m > bm || (m == bm && i < bi)
            });
            if better {
                best = Some(s);
            }
        }
        best.ok_or_else(|| {
            PinError::DeviceType(format!("{} supports no iPhone device type", rt.name))
        })?
    };
    Ok(Choice {
        device_type: device.identifier.clone(),
        runtime: rt.identifier.clone(),
    })
}

/// The name a Task's simulator is created with: `moor-<key>`.
#[must_use]
pub fn device_name(key: &TaskKey) -> String {
    format!("moor-{}", key.as_str())
}

/// Shell text that creates a simulator called `name` in the device set at `set`.
///
/// It prints the new simulator's UDID on stdout.
#[must_use]
pub fn create_script(set: &BoxPath, name: &str, c: &Choice) -> String {
    format!(
        "xcrun simctl --set {} create {} {} {}",
        quote(set.as_str()),
        quote(name),
        quote(&c.device_type),
        quote(&c.runtime)
    )
}

/// Reads the UDID `simctl create` printed: the last non-empty line, if it has the
/// uppercase 8-4-4-4-12 hex shape.
#[must_use]
pub fn parse_udid(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let groups: Vec<&str> = line.split('-').collect();
    let shape = [8, 4, 4, 4, 12];
    let ok = groups.len() == shape.len()
        && groups.iter().zip(shape).all(|(g, n)| {
            g.len() == n
                && g.bytes()
                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        });
    ok.then(|| line.to_owned())
}

#[cfg(test)]
mod tests;
