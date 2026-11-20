//! The images check: whether the Box images the installed agent will pull exist. The unit
//! `moor up` wrote holds what the agent runs with, so the check reads that, not the shell.

use std::fs;
use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use moor_core::seam::clock::Clock as _;

use super::{Check, Inputs};
use crate::adapters::clock::SystemClock;
use crate::up::service;

/// How long a registry lookup may take before the check calls it unknown.
const MANIFEST_WAIT: Duration = Duration::from_secs(10);
/// The fix for an image that is nowhere.
const FIX: &str = "set MOOR_AGENT_IMAGE and MOOR_WORK_IMAGE, such as to the images of a nightly release, then rerun moor up";

/// Where an image is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Presence {
    /// In this host's image store.
    Local,
    /// In its registry.
    Pullable,
    /// In neither.
    Missing,
    /// The lookup failed, such as with no network, so nothing is known.
    Unknown(String),
}

/// One line for each image of the installed agent; none when this host has no agent unit.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    let Some(dir) = service::unit_dir(i.env) else {
        return Vec::new();
    };
    let Ok(unit) = fs::read_to_string(dir.join(service::AGENT)) else {
        return Vec::new();
    };
    let version = unit_exe(&unit)
        .and_then(|exe| (i.agent_version)(exe))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let wanted = [
        ("agent", "moor-agent", "MOOR_AGENT_IMAGE"),
        ("work", "moor-work", "MOOR_WORK_IMAGE"),
    ]
    .map(|(role, name, var)| {
        let image =
            unit_var(&unit, var).unwrap_or_else(|| format!("ghcr.io/inizio/{name}:{version}"));
        (role, image)
    });
    let present = i.present;
    let found: Vec<Presence> = std::thread::scope(|s| {
        let probes: Vec<_> = wanted
            .iter()
            .map(|(_, image)| s.spawn(|| present(image)))
            .collect();
        probes
            .into_iter()
            .map(|p| {
                p.join()
                    .unwrap_or_else(|_| Presence::Unknown("the lookup panicked".to_owned()))
            })
            .collect()
    });
    wanted
        .iter()
        .zip(found)
        .map(|((role, image), found)| line(role, image, found, i.images_fail))
        .collect()
}

/// The check for the `role` image `image`, which was `found`. A missing image fails when `strict`.
fn line(role: &str, image: &str, found: Presence, strict: bool) -> Check {
    match found {
        Presence::Local => Check::ok(
            "images",
            format!("the {role} image {image} is on this host"),
        ),
        Presence::Pullable => {
            Check::ok("images", format!("the {role} image {image} can be pulled"))
        }
        Presence::Missing => {
            let detail = format!(
                "the {role} image {image} is not on this host and not in its registry, so the first Box cannot start"
            );
            if strict {
                Check::fail("images", detail, FIX)
            } else {
                Check::warn("images", detail, FIX)
            }
        }
        Presence::Unknown(why) => Check::warn(
            "images",
            format!("cannot tell whether the {role} image {image} can be pulled: {why}"),
            "check the network; the first Box pulls the image",
        ),
    }
}

/// The value of `name` in the unit's `Environment=` lines.
fn unit_var(unit: &str, name: &str) -> Option<String> {
    let prefix = format!("Environment=\"{name}=");
    unit.lines()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The executable the unit's `ExecStart=` runs.
fn unit_exe(unit: &str) -> Option<&str> {
    let rest = unit.lines().find_map(|l| l.strip_prefix("ExecStart=\""))?;
    rest.split_once('"').map(|(exe, _)| exe)
}

/// The version the agent at `exe` reports with `--version`, which names its default images.
pub(crate) fn agent_version(exe: &str) -> Option<String> {
    let out = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .map(str::to_owned)
}

/// Where `image` is: this host's store, else its registry, through the `docker` command line.
pub(crate) fn present(image: &str) -> Presence {
    let local = Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if local.is_ok_and(|s| s.success()) {
        return Presence::Local;
    }
    match manifest(image) {
        Ok(()) => Presence::Pullable,
        Err(said) => classify_manifest(&said),
    }
}

/// `docker manifest inspect image`, which asks the registry without pulling; else what it said.
fn manifest(image: &str) -> Result<(), String> {
    let mut child = Command::new("docker")
        .args(["manifest", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    let clock = SystemClock::real();
    let deadline = clock.instant() + MANIFEST_WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if clock.instant() < deadline => clock.sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", MANIFEST_WAIT.as_secs()));
            }
            Err(e) => return Err(format!("cannot wait for docker: {e}")),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut said = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut said);
    }
    Err(said)
}

/// What a failed `docker manifest inspect` that said `stderr` means: the registry's own "no
/// such tag" answers are missing; a network error or anything else is unknown.
pub(crate) fn classify_manifest(stderr: &str) -> Presence {
    let lower = stderr.to_lowercase();
    if ["no such manifest", "manifest unknown", "name unknown"]
        .iter()
        .any(|m| lower.contains(m))
    {
        return Presence::Missing;
    }
    let first = stderr.lines().map(str::trim).find(|l| !l.is_empty());
    Presence::Unknown(first.unwrap_or("docker manifest inspect failed").to_owned())
}
