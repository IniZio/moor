//! What sync changes so hc has one machine per up Box.
//!
//! The plan is pure: the up Boxes and the machines hc lists in, steps out. Machines whose via
//! command moor-hc did not write for this controller are never removed and keep their names.

use std::collections::BTreeSet;
use std::fmt;

use crate::via::{Link, Via};

/// Longest machine name hc accepts.
const NAME_MAX: usize = 32;

/// Hex digits of the hash that tells apart names cut to fit or already taken.
const HASH_HEX: usize = 6;

/// One machine as hc lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// hc's name for it.
    pub name: String,
    /// Its via command, when moor-hc wrote it.
    pub via: Option<Via>,
}

/// One change to hc's machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Forget a machine moor-hc added: its Box is no longer up, or it runs an old binary.
    Remove {
        /// The machine.
        name: String,
    },
    /// Add a machine reached through `via`.
    Add {
        /// The machine's name.
        name: String,
        /// How hc reaches the Box.
        via: Via,
    },
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remove { name } => write!(f, "{name}: removed"),
            Self::Add { name, via } => write!(f, "{name}: added for {}", via.box_name),
        }
    }
}

/// The steps that leave one machine per Box in `boxes`, reached by `exe` over `link`. Removals
/// come first, so a name they free can be taken again.
#[must_use]
pub fn plan(exe: &str, link: &Link, boxes: &BTreeSet<String>, machines: &[Machine]) -> Vec<Step> {
    let mut covered = BTreeSet::new();
    let mut steps = Vec::new();
    let mut taken = BTreeSet::new();
    for m in machines {
        let Some(via) = m.via.as_ref().filter(|v| v.link == *link) else {
            taken.insert(m.name.clone());
            continue;
        };
        let keep =
            via.exe == exe && boxes.contains(&via.box_name) && covered.insert(via.box_name.clone());
        if keep {
            taken.insert(m.name.clone());
        } else {
            steps.push(Step::Remove {
                name: m.name.clone(),
            });
        }
    }
    for box_name in boxes.iter().filter(|b| !covered.contains(*b)) {
        let name = machine_name(link, box_name, &taken);
        taken.insert(name.clone());
        steps.push(Step::Add {
            name,
            via: Via {
                exe: exe.to_owned(),
                link: link.clone(),
                box_name: box_name.clone(),
            },
        });
    }
    steps
}

/// The machine name for `box_name`: the name with `/` as `.`, which hc accepts as is. One too
/// long, shaped like an hc terminal id or already taken is cut and given a hash of the
/// controller and Box.
#[must_use]
pub fn machine_name(link: &Link, box_name: &str, taken: &BTreeSet<String>) -> String {
    let plain = box_name.replace('/', ".");
    if plain.len() <= NAME_MAX && !id_shaped(&plain) && !taken.contains(&plain) {
        return plain;
    }
    let stem: String = plain.chars().take(NAME_MAX - HASH_HEX - 1).collect();
    let stem = stem.trim_end_matches(['.', '-']);
    (0..u32::MAX)
        .map(|n| {
            let seed = format!("{}\n{}\n{box_name}\n{n}", link.flag().0, link.flag().1);
            format!(
                "{stem}-{:0w$x}",
                fnv(&seed) >> (64 - 4 * HASH_HEX),
                w = HASH_HEX
            )
        })
        .find(|name| !taken.contains(name))
        .unwrap_or(plain)
}

/// hc refuses names shaped like its terminal ids: a letter, then digits.
fn id_shaped(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() > 1
        && chars.all(|c| c.is_ascii_digit())
}

/// The 64-bit FNV-1a hash of `s`; stable across builds, unlike the standard hasher.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests;
