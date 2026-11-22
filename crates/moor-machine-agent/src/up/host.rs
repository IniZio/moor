//! Makes a Linux host ready for Docker Boxes before `up` checks it. What only root can fix goes
//! in one sudo command the person agrees to; then rootless Docker is set up as the person, with
//! Docker's own setup tool, or Docker's rootless installer where Docker's package is not at hand.

use std::path::PathBuf;

use moor_client::{kvm, rootless};

mod daemon;
mod probe;
#[cfg(test)]
mod tests;

#[cfg(test)]
use probe::Ran;
pub(crate) use probe::{Real, Shell};

/// The ids a new range gives the person, from the first id no one has.
const SUBIDS: u64 = 65_536;
/// Where new id ranges start when no one has any.
const SUBID_START: u64 = 100_000;
/// The one question `up` asks before it runs anything as root.
pub(crate) const QUESTION: &str = "Run these with sudo now? [Y/n] ";
/// How each failure ends, so the person knows the way back.
pub(super) const AGAIN: &str = "then run moor-machine-agent up again";

/// A package manager `up` knows the package names for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Packager {
    /// Debian and Ubuntu.
    Apt,
    /// Fedora and RHEL.
    Dnf,
}

/// Where the person's rootless daemon stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Daemon {
    /// It answers at the person's socket.
    Answers,
    /// Its user unit is installed; `active` when systemd runs it.
    Installed {
        /// Whether `systemctl --user is-active docker` says active.
        active: bool,
    },
    /// Not set up.
    Absent,
}

/// What the host says, before anything changes it.
#[derive(Debug, Clone)]
pub(crate) struct Facts {
    /// What the rootless preflight reads.
    pub(crate) rootless: rootless::Facts,
    /// The KVM device the checks read.
    pub(crate) kvm_device: String,
    /// Its permission bits, when it exists.
    pub(crate) kvm_mode: Option<u32>,
    /// The host's package manager, when `up` knows it.
    pub(crate) packager: Option<Packager>,
    /// Whether Docker's own package repository is configured.
    pub(crate) docker_repo: bool,
    /// Docker's setup tool, where one is installed.
    pub(crate) setuptool: Option<PathBuf>,
    /// The person's rootless daemon.
    pub(crate) daemon: Daemon,
    /// The person's home.
    pub(crate) home: PathBuf,
    /// `XDG_CONFIG_HOME`, where the daemon's unit and daemon.json live.
    pub(crate) config: PathBuf,
    /// The docker CLI's config directory, which names its current context.
    pub(crate) docker_config: PathBuf,
    /// The rootless daemon's socket.
    pub(crate) socket: PathBuf,
}

/// One thing only root can fix, and the commands that fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Gap {
    /// What is wrong, in plain words.
    pub(crate) why: String,
    /// The shell commands that fix it, run as root.
    pub(crate) commands: Vec<String>,
}

/// Makes the host `read` describes ready for rootless Docker: asks once, through `ask`, to run
/// every root-only fix in one sudo command (none means no terminal to ask on), then sets up and
/// starts the person's rootless daemon, telling `say` one line per step.
///
/// # Errors
/// A gap no command fixes, no terminal or a no to the question, a failed sudo, or a failed
/// setup; each names its fix.
pub(crate) fn prepare(
    read: &dyn Fn(&mut dyn Shell) -> Result<Facts, String>,
    shell: &mut dyn Shell,
    ask: Option<&mut dyn FnMut(&str) -> bool>,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut facts = read(shell)?;
    if facts.rootless.linger == Some(false) && shell.run("loginctl", &["enable-linger"], &[]).ok {
        facts.rootless.linger = Some(true);
        say("linger: turned on");
    }
    let found = gaps(&facts)?;
    if found.is_empty() {
        say("host: ok");
    } else {
        root(&found, shell, ask, say)?;
        facts = read(shell)?;
        let left = gaps(&facts)?;
        if !left.is_empty() {
            return Err(format!(
                "sudo ran, but this host still needs root for:\n{}\nfix each, {AGAIN}",
                list(&left)
            ));
        }
        say("host: fixed with sudo");
    }
    daemon::install(&facts, shell, say)
}

/// Every root-only gap of `f`, in the order the sudo command fixes them.
///
/// # Errors
/// A gap no command fixes: no KVM device, or packages and no known package manager.
pub(crate) fn gaps(f: &Facts) -> Result<Vec<Gap>, String> {
    let failed = rootless::failures(&f.rootless, false);
    let failure = |check: &str| failed.iter().find(|x| x.check == check);
    let mut found = Vec::new();
    found.extend(kvm_gap(f)?);
    found.extend(package_gap(f)?);
    found.extend(subid_gap(&f.rootless));
    if let Some(x) = failure("apparmor") {
        found.push(Gap {
            why: x.detail.clone(),
            commands: vec![
                "echo kernel.apparmor_restrict_unprivileged_userns=0 > /etc/sysctl.d/60-rootless-docker.conf".to_owned(),
                "sysctl -w kernel.apparmor_restrict_unprivileged_userns=0".to_owned(),
            ],
        });
    }
    if let Some(x) = failure("linger") {
        found.push(Gap {
            why: x.detail.clone(),
            commands: vec![format!("loginctl enable-linger {}", f.rootless.user)],
        });
    }
    Ok(found)
}

/// The KVM device opened to every user.
fn kvm_gap(f: &Facts) -> Result<Option<Gap>, String> {
    let Some(x) = kvm::refusal(&f.kvm_device, f.kvm_mode) else {
        return Ok(None);
    };
    if f.kvm_mode.is_none() {
        return Err(format!("{}: {}; fix: {}", x.check, x.detail, x.fix));
    }
    Ok(Some(Gap {
        why: x.detail,
        commands: vec![
            r#"echo 'KERNEL=="kvm", MODE="0666"' > /etc/udev/rules.d/65-kvm.rules"#.to_owned(),
            "udevadm control --reload".to_owned(),
            "udevadm trigger --name-match=kvm".to_owned(),
            "udevadm settle".to_owned(),
        ],
    }))
}

/// The packages rootless Docker needs that are not installed.
fn package_gap(f: &Facts) -> Result<Option<Gap>, String> {
    let dnf = f.packager == Some(Packager::Dnf);
    let mut packages: Vec<&str> = Vec::new();
    for bin in &f.rootless.missing {
        let package = match bin.as_str() {
            "newuidmap" | "newgidmap" if dnf => "shadow-utils",
            "newuidmap" | "newgidmap" => "uidmap",
            "slirp4netns" => "slirp4netns",
            _ => continue,
        };
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    if f.daemon == Daemon::Absent && f.setuptool.is_none() && f.docker_repo {
        packages.push("docker-ce-rootless-extras");
    }
    if packages.is_empty() {
        return Ok(None);
    }
    let names = packages.join(" ");
    let commands = match f.packager {
        Some(Packager::Apt) => vec![
            "apt-get update -qq".to_owned(),
            format!("apt-get install -y {names}"),
        ],
        Some(Packager::Dnf) => vec![format!("dnf install -y {names}")],
        None => {
            return Err(format!(
                "rootless Docker needs {names}, and this host has neither apt-get nor dnf; install \
                 them with its package manager, {AGAIN}"
            ))
        }
    };
    Ok(Some(Gap {
        why: format!(
            "rootless Docker needs these packages: {}",
            packages.join(", ")
        ),
        commands,
    }))
}

/// More subordinate ids for the person, from the first id no one has, in each short file.
fn subid_gap(f: &rootless::Facts) -> Option<Gap> {
    let files = [("--add-subuids", &f.subuid), ("--add-subgids", &f.subgid)];
    let short: Vec<&str> = files
        .iter()
        .filter(|(_, text)| {
            rootless::subid_count(text.as_deref().unwrap_or_default(), &f.user, f.uid)
                < rootless::SUBIDS_MIN
        })
        .map(|(flag, _)| *flag)
        .collect();
    if short.is_empty() {
        return None;
    }
    let start = files
        .iter()
        .flat_map(|(_, text)| text.as_deref().unwrap_or_default().lines())
        .filter_map(|line| {
            let mut field = line.split(':').skip(1);
            let first: u64 = field.next()?.trim().parse().ok()?;
            Some(first + field.next()?.trim().parse::<u64>().ok()?)
        })
        .fold(SUBID_START, u64::max);
    let range = format!("{start}-{}", start + SUBIDS - 1);
    let flags: Vec<String> = short.iter().map(|flag| format!("{flag} {range}")).collect();
    Some(Gap {
        why: format!(
            "{} has fewer than {} subordinate ids, which rootless Docker maps its containers to",
            f.user,
            rootless::SUBIDS_MIN
        ),
        commands: vec![format!("usermod {} {}", flags.join(" "), f.user)],
    })
}

/// The gaps as lines of a list.
fn list(found: &[Gap]) -> String {
    found
        .iter()
        .map(|g| format!("  - {}", g.why))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Shows every root-only fix as one sudo command and runs it once the person says yes.
fn root(
    found: &[Gap],
    shell: &mut dyn Shell,
    ask: Option<&mut dyn FnMut(&str) -> bool>,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let script = found
        .iter()
        .flat_map(|g| g.commands.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    say(&format!(
        "this host needs root for:\n{}\nthis one sudo command fixes them:\nsudo sh -ec {}",
        list(found),
        quote(&script)
    ));
    let again = format!("run the sudo command above, {AGAIN}");
    let Some(ask) = ask else {
        return Err(format!("no terminal to ask on, so nothing ran; {again}"));
    };
    if !ask(QUESTION) {
        return Err(format!("nothing ran; {again}"));
    }
    if !shell.sudo(&script) {
        return Err(format!("sudo failed; fix the error above, {AGAIN}"));
    }
    Ok(())
}

/// `text` as one single-quoted shell word.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
