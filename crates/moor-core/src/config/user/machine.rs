//! Machines a moor user enrolls, each reached over the link its `moor-machine-agent` dials.

use serde::{Deserialize, Serialize};

pub use pick::{pick_machine, unslot_ports, MachinePick};

mod pick;

/// Longest machine name.
const NAME_MAX: usize = 64;
/// Longest label.
const LABEL_MAX: usize = 32;
/// Most labels on one machine.
const LABELS_MAX: usize = 16;

/// One of a user's enrolled machines, without the key its agent signs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMachine {
    /// The machine's name, unique per user.
    pub name: String,
    /// What the machine offers, such as `macos`; sorted and without repeats.
    pub labels: Vec<String>,
    /// How many Boxes may hold the machine at once, each in its own port slot; at least 1.
    #[serde(default = "one")]
    pub capacity: u16,
    /// The crabbox provider this machine offers Boxes on, when it hosts a Box pool.
    #[serde(default)]
    pub pool: Option<String>,
}

/// The capacity of a machine added without one.
const fn one() -> u16 {
    1
}

impl UserMachine {
    /// Checks a machine's name and labels.
    ///
    /// # Errors
    /// Why the name or a label is not accepted.
    pub fn parse(name: &str, labels: &[String]) -> Result<Self, String> {
        check_name(name)?;
        let mut labels = labels.to_vec();
        labels.sort();
        labels.dedup();
        if labels.len() > LABELS_MAX {
            return Err(format!("at most {LABELS_MAX} labels"));
        }
        labels.iter().try_for_each(|l| check_label(l))?;
        Ok(Self {
            name: name.to_owned(),
            labels,
            capacity: 1,
            pool: None,
        })
    }

    /// The machine as a Box pool host offering Boxes through crabbox provider `pool`.
    ///
    /// # Errors
    /// When `pool` is not a crabbox provider name.
    pub fn with_pool(self, pool: &str) -> Result<Self, String> {
        check_pool(pool)?;
        Ok(Self {
            pool: Some(pool.to_owned()),
            ..self
        })
    }

    /// The machine taking up to `capacity` Boxes at once.
    ///
    /// # Errors
    /// When `capacity` is 0.
    pub fn with_capacity(self, capacity: u16) -> Result<Self, String> {
        if capacity == 0 {
            return Err("capacity must be at least 1".to_owned());
        }
        Ok(Self { capacity, ..self })
    }
}

/// A machine its agent enrolled, with the key the agent signs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledMachine {
    /// The machine's name, unique per user.
    pub name: String,
    /// The agent's ed25519 public key as an OpenSSH line, `ssh-ed25519 AAAA...`.
    pub pubkey: String,
    /// The machine's labels, sorted and distinct.
    pub labels: Vec<String>,
    /// How many Boxes may hold it at once.
    pub capacity: u16,
    /// The crabbox provider it offers Boxes on, when it hosts a Box pool.
    pub pool: Option<String>,
}

impl EnrolledMachine {
    /// Checks a machine an agent enrolls.
    ///
    /// # Errors
    /// Why the name, key, a label or the capacity is not accepted.
    pub fn parse(
        name: &str,
        pubkey: &str,
        labels: &[String],
        capacity: u16,
    ) -> Result<Self, String> {
        check_pubkey(pubkey)?;
        let checked = UserMachine::parse(name, labels)?.with_capacity(capacity)?;
        Ok(Self {
            name: checked.name,
            pubkey: pubkey.to_owned(),
            labels: checked.labels,
            capacity: checked.capacity,
            pool: None,
        })
    }

    /// The machine as a Box pool host offering Boxes through crabbox provider `pool`.
    ///
    /// # Errors
    /// When `pool` is not a crabbox provider name.
    pub fn with_pool(self, pool: &str) -> Result<Self, String> {
        check_pool(pool)?;
        Ok(Self {
            pool: Some(pool.to_owned()),
            ..self
        })
    }
}

/// Longest crabbox provider name.
const POOL_MAX: usize = 64;

/// `[a-z][a-z0-9-]*` in at most 64 bytes, as crabbox names a provider.
///
/// # Errors
/// Why `pool` is not accepted, naming `--pool`.
fn check_pool(pool: &str) -> Result<(), String> {
    let ok = pool.len() <= POOL_MAX
        && pool.starts_with(|c: char| c.is_ascii_lowercase())
        && pool
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "--pool provider `{pool}` must match [a-z][a-z0-9-]* in at most {POOL_MAX} bytes"
        ))
    }
}

/// `ssh-ed25519 <base64>` with nothing after it. Every ed25519 key blob is 51 bytes that
/// start with the same 19, so its base64 is 68 characters with this fixed head.
fn check_pubkey(pubkey: &str) -> Result<(), String> {
    const HEAD: &str = "AAAAC3NzaC1lZDI1NTE5AAAAI";
    let ok = pubkey.strip_prefix("ssh-ed25519 ").is_some_and(|b64| {
        b64.len() == 68
            && b64.starts_with(HEAD)
            && b64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    });
    if ok {
        Ok(())
    } else {
        Err(
            "the machine key must be one OpenSSH ed25519 public key, `ssh-ed25519 AAAA...`"
                .to_owned(),
        )
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

#[cfg(test)]
mod tests;
