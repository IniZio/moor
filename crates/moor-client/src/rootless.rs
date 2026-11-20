//! Rootless Docker: how `moor up` and `moor-machine-agent up` tell it from rootful Docker, and
//! what they check before they trust it. Nothing here changes the host.

use std::path::{Path, PathBuf};

mod probe;
#[cfg(test)]
mod tests;

pub use probe::{child_ip, daemon_state, detect, Facts};
/// Docker's security options as JSON; `name=rootless` is among them under rootless Docker.
pub const SECURITY_ARGS: [&str; 3] = ["info", "--format", "{{json .SecurityOptions}}"];

/// Whether the daemon runs with live-restore, as `true` or `false`.
const LIVE_RESTORE_ARGS: [&str; 3] = ["info", "--format", "{{.LiveRestoreEnabled}}"];

/// Ids a user needs in each of `/etc/subuid` and `/etc/subgid`.
const SUBIDS_MIN: u64 = 65_536;
/// The flag that closes the host's loopback to every Box.
const LOOPBACK_FLAG: &str = "--disable-host-loopback";

/// Whether `options`, the JSON of `docker info`'s `SecurityOptions`, name rootless Docker.
#[must_use]
pub fn is_rootless(options: &str) -> bool {
    serde_json::from_str::<Vec<String>>(options)
        .unwrap_or_default()
        .iter()
        .any(|o| o.split(',').any(|field| field == "name=rootless"))
}

/// `docker info`'s live-restore line as a flag; none when it says neither `true` nor `false`.
fn live_restore_of(line: &str) -> Option<bool> {
    match line.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// What the notice about Docker Boxes says to `user` under rootless Docker.
#[must_use]
pub fn notice(user: &str) -> String {
    format!(
        "Docker Boxes run on rootless Docker as {user}. Each Box's work place runs with \
         --privileged so Docker works inside it, and a Box that breaks out is {user} on this \
         host, not root. It can reach {user}'s files and every other Box here."
    )
}

/// What to tell the user about `failed`: a heading, then one line for each with its fix.
#[must_use]
pub fn report(failed: &[Failure]) -> String {
    let lines = failed
        .iter()
        .map(|f| format!("- {}: {}; fix: {}", f.check, f.detail, f.fix));
    std::iter::once("rootless Docker is not ready for Boxes:".to_owned())
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One preflight check that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What was checked.
    pub check: &'static str,
    /// What was found.
    pub detail: String,
    /// What to do about it.
    pub fix: String,
}

/// A failed check of `check`.
fn fail(check: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Failure {
    Failure {
        check,
        detail: detail.into(),
        fix: fix.into(),
    }
}

/// Every preflight check that fails for `facts`. `local_relay` is true where a relay will
/// enter the daemon's namespace, so `nsenter` is needed.
#[must_use]
pub fn failures(facts: &Facts, local_relay: bool) -> Vec<Failure> {
    let mut found = Vec::new();
    found.extend(subids(facts));
    found.extend(tools(facts, local_relay));
    found.extend(apparmor(facts));
    found.extend(unit(facts));
    found.extend(linger(facts));
    found.extend(loopback(facts));
    found
}

/// Every finding for `facts` that leaves Docker Boxes working, so `up` says it and goes on.
#[must_use]
pub fn warnings(facts: &Facts) -> Vec<Failure> {
    kvm(facts).into_iter().chain(live_restore(facts)).collect()
}

/// The user's ids in `file` for the name or uid `user`: every range counts.
fn subid_count(file: &str, user: &str, uid: u32) -> u64 {
    file.lines()
        .filter_map(|line| {
            let mut f = line.split(':');
            let (owner, _, count) = (f.next()?, f.next()?, f.next()?);
            (owner == user || owner == uid.to_string()).then(|| count.trim().parse::<u64>().ok())?
        })
        .sum()
}

/// Both id files give the user enough ids.
fn subids(f: &Facts) -> Vec<Failure> {
    [
        ("/etc/subuid", &f.subuid, "--add-subuids"),
        ("/etc/subgid", &f.subgid, "--add-subgids"),
    ]
    .into_iter()
    .filter_map(|(path, text, flag)| {
        let have = text
            .as_deref()
            .map_or(0, |t| subid_count(t, &f.user, f.uid));
        (have < SUBIDS_MIN).then(|| {
            fail(
                "subid",
                format!(
                    "{path} gives {} {have} ids, and rootless Docker needs {SUBIDS_MIN}",
                    f.user
                ),
                format!("run `sudo usermod {flag} 100000-165535 {}`", f.user),
            )
        })
    })
    .collect()
}

/// The programs rootless Docker, and a relay, run.
fn tools(f: &Facts, local_relay: bool) -> Vec<Failure> {
    let needs = |bin: &str| local_relay || bin != "nsenter";
    [
        ("newuidmap", "uidmap"),
        ("newgidmap", "uidmap"),
        ("slirp4netns", "slirp4netns"),
        ("nsenter", "util-linux"),
    ]
    .into_iter()
    .filter(|(bin, _)| needs(bin) && f.missing.iter().any(|m| m == bin))
    .map(|(bin, package)| {
        fail(
            "tools",
            format!("{bin} is not installed"),
            format!("install {package}"),
        )
    })
    .collect()
}

/// `AppArmor` lets an unprivileged user make a user namespace; a host without the sysctl does.
fn apparmor(f: &Facts) -> Option<Failure> {
    let value = f.apparmor_restrict.as_deref()?.trim();
    (value != "0").then(|| {
        fail(
            "apparmor",
            format!("kernel.apparmor_restrict_unprivileged_userns is {value}"),
            "run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`, and set it in /etc/sysctl.d to keep it",
        )
    })
}

/// The rootless `docker.service` user unit starts with the user's manager.
fn unit(f: &Facts) -> Option<Failure> {
    let state = f
        .unit
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    (state != "enabled").then(|| {
        fail(
            "docker.service",
            format!("the docker.service user unit is {state}, so the daemon will not come back after a reboot"),
            "run `systemctl --user enable docker`",
        )
    })
}

/// The user's services outlive their login; unknown counts as yes.
fn linger(f: &Facts) -> Option<Failure> {
    (f.linger == Some(false)).then(|| {
        fail(
            "linger",
            format!("{} does not linger, so the daemon stops at logout", f.user),
            "run `loginctl enable-linger`",
        )
    })
}

/// rootlesskit runs with the flag that closes the host's loopback to every Box.
fn loopback(f: &Facts) -> Option<Failure> {
    let fix = "unset DOCKERD_ROOTLESS_ROOTLESSKIT_DISABLE_HOST_LOOPBACK, then restart the daemon";
    match &f.rootlesskit {
        Err(why) => Some(fail("loopback", why.clone(), fix)),
        Ok(args) if loopback_closed(args) => None,
        Ok(_) => Some(fail(
            "loopback",
            "rootlesskit runs without --disable-host-loopback, so every Box reaches every \
             127.0.0.1 listener of this host",
            fix,
        )),
    }
}

/// Whether `args`, rootlesskit's own, carry the flag that closes the host's loopback.
fn loopback_closed(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == LOOPBACK_FLAG || a == &format!("{LOOPBACK_FLAG}=true"))
}

/// A node that offers KVM, as any host with `/dev/kvm` does, can open it from the work place. A
/// node that cannot still runs every Box but a KVM one, so this is a warning, not a failure.
fn kvm(f: &Facts) -> Option<Failure> {
    let mode = f.kvm_mode?;
    (mode & 0o666 != 0o666).then(|| {
        fail(
            "kvm",
            format!(
                "KVM Boxes (Android emulators) will not work on this node until /dev/kvm is 0666; \
                 it is mode {mode:04o}, and rootless Docker drops its group"
            ),
            "add a udev rule with MODE=\"0666\" for KERNEL==\"kvm\", such as in /etc/udev/rules.d/65-kvm.rules",
        )
    })
}

/// Rootlesskit makes a new network namespace per daemon, so live-restore cannot adopt old containers.
fn live_restore(f: &Facts) -> Option<Failure> {
    (f.live_restore == Some(true)).then(|| {
        fail(
            "live-restore",
            "live-restore is on, and a restart of the rootless daemon then leaves Box processes in \
             the old network namespace, so `moor wake` fails with \"container with given ID already \
             exists\"",
            "turn live-restore off in ~/.config/docker/daemon.json, then restart the daemon",
        )
    })
}

/// The directory rootlesskit keeps its state in for the daemon at `host`, a `unix://` address.
///
/// # Errors
/// The daemon is not at a Unix socket.
pub fn state_dir(host: &str) -> Result<PathBuf, String> {
    let socket = host
        .strip_prefix("unix://")
        .ok_or_else(|| format!("the daemon is at {host}, not a Unix socket on this host"))?;
    Path::new(socket)
        .parent()
        .map(|dir| dir.join("dockerd-rootless"))
        .ok_or_else(|| format!("{socket} has no directory"))
}

/// The address of the rootlesskit namespace's own network device, from the JSON that
/// rootlesskit's `api.sock` answers at `/v1/info`.
///
/// # Errors
/// The text is not rootlesskit's info, or its network driver reports no address.
pub fn child_ip_from(json: &str) -> Result<std::net::Ipv4Addr, String> {
    let info: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("rootlesskit's info is not JSON: {e}"))?;
    let ip = info
        .pointer("/networkDriver/childIP")
        .and_then(serde_json::Value::as_str)
        .ok_or("rootlesskit's network driver reports no childIP, so the relay has no address")?;
    ip.parse()
        .map_err(|_| format!("rootlesskit reports childIP {ip}, which is not an IPv4 address"))
}

/// The words of a `/proc/<pid>/cmdline`.
fn cmdline_words(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|w| !w.is_empty())
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect()
}
