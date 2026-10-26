//! Machines a moor user registers for themselves, reached over ssh with the host's own key.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

pub use pick::{pick_machine, MachinePick};

mod pick;

/// Longest machine name.
const NAME_MAX: usize = 64;
/// Longest label.
const LABEL_MAX: usize = 32;
/// Most labels on one machine.
const LABELS_MAX: usize = 16;
/// Longest ssh user.
const USER_MAX: usize = 32;
/// Longest host name.
const HOST_MAX: usize = 253;

/// One of a user's machines. It holds no credential: the host's own ssh key or agent reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMachine {
    /// The machine's name, unique per user.
    pub name: String,
    /// The ssh target, `user@host` or `user@host:port`.
    pub ssh: String,
    /// What the machine offers, such as `macos`; sorted and without repeats.
    pub labels: Vec<String>,
}

impl UserMachine {
    /// Checks a machine a user registers.
    ///
    /// # Errors
    /// Why the name, ssh target or a label is not accepted.
    pub fn parse(name: &str, ssh: &str, labels: &[String]) -> Result<Self, String> {
        check_name(name)?;
        check_ssh(ssh)?;
        let mut labels = labels.to_vec();
        labels.sort();
        labels.dedup();
        if labels.len() > LABELS_MAX {
            return Err(format!("at most {LABELS_MAX} labels"));
        }
        labels.iter().try_for_each(|l| check_label(l))?;
        Ok(Self {
            name: name.to_owned(),
            ssh: ssh.to_owned(),
            labels,
        })
    }
}

/// `[a-z][a-z0-9_-]*` in at most 64 bytes.
fn check_name(name: &str) -> Result<(), String> {
    let ok = name.len() <= NAME_MAX
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "machine name `{name}` must match [a-z][a-z0-9_-]* in at most {NAME_MAX} bytes"
        ))
    }
}

/// `[a-z0-9][a-z0-9-]*` in at most 32 bytes.
pub(crate) fn check_label(label: &str) -> Result<(), String> {
    let ok = label.len() <= LABEL_MAX
        && label.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "label `{label}` must match [a-z0-9][a-z0-9-]* in at most {LABEL_MAX} bytes"
        ))
    }
}

/// `user@host` or `user@host:port`; the host is a DNS name, an IPv4 address or a bracketed IPv6
/// address. Nothing in it can read as a command-line option or reach a shell.
fn check_ssh(target: &str) -> Result<(), String> {
    let bad = || format!("ssh target `{target}` must be user@host or user@host:port");
    let (user, rest) = target.split_once('@').ok_or_else(bad)?;
    let user_ok = !user.is_empty()
        && user.len() <= USER_MAX
        && user.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    let (host, port) = split_port(rest).ok_or_else(bad)?;
    let port_ok = port.is_none_or(|p| {
        p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && matches!(p.parse::<u16>(), Ok(n) if n > 0)
    });
    if user_ok && port_ok && check_host(host) {
        Ok(())
    } else {
        Err(bad())
    }
}

/// Splits `host[:port]`; an IPv6 host keeps its brackets.
fn split_port(rest: &str) -> Option<(&str, Option<&str>)> {
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        let host = rest.get(..=end)?;
        return match rest.get(end + 1..)? {
            "" => Some((host, None)),
            tail => Some((host, Some(tail.strip_prefix(':')?))),
        };
    }
    match rest.split_once(':') {
        Some((host, port)) => Some((host, Some(port))),
        None => Some((rest, None)),
    }
}

/// A DNS name, an IPv4 address or a bracketed IPv6 address.
fn check_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<Ipv6Addr>().is_ok();
    }
    if host.is_empty() || host.len() > HOST_MAX {
        return false;
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return host.parse::<Ipv4Addr>().is_ok();
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

#[cfg(test)]
mod tests;
