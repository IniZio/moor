//! Makes herdr's workspaces match the open Boxes. Running it again changes nothing, and a
//! Box that closed keeps its workspace, marked closed, until the user closes it.

use crate::boxes::{Phase, Shown, BOX_MARK};
use crate::error::Error;
use crate::herdr::{Herdr, Space};

/// The token every Box workspace carries, so the sidebar config can colour Boxes.
pub const ENV_TOKEN: (&str, &str) = ("env", "BOX");

/// The tokens a Box workspace in `phase` carries.
#[must_use]
pub const fn tokens(phase: Phase) -> [(&'static str, &'static str); 2] {
    [ENV_TOKEN, ("state", phase.icon())]
}

/// Whether `space` already carries `want`; herdr drops an empty token, so empty matches absent.
fn carries(space: &Space, want: &[(&str, &str)]) -> bool {
    want.iter()
        .all(|(k, v)| space.tokens.get(*k).map_or("", String::as_str) == *v)
}

/// Brings one existing workspace to `phase`.
fn settle(herdr: &mut dyn Herdr, space: &Space, phase: Phase) -> Result<bool, Error> {
    let want = tokens(phase);
    let retagged = !carries(space, &want);
    if retagged {
        herdr.tag(&space.id, &want)?;
    }
    let mut changed = retagged;
    if space.agent_status != phase.agent_state() {
        if let Some(pane) = herdr.first_pane(&space.id)? {
            herdr.report_agent(&pane, phase.agent_state())?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Reconciles herdr with `boxes`. A new Box gets a workspace whose pane runs `attach(name)`.
/// Returns one line per change, for the log.
pub fn reconcile(
    herdr: &mut dyn Herdr,
    boxes: &[Shown],
    attach: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, Error> {
    let spaces = herdr.spaces()?;
    let mut report = Vec::new();
    for b in boxes {
        let label = b.label();
        if let Some(space) = spaces.iter().find(|s| s.label == label) {
            if settle(herdr, space, b.phase)? {
                report.push(format!("{label}: {}", b.phase.agent_state()));
            }
            continue;
        }
        let made = herdr.create(&label)?;
        herdr.run(&made.pane, &attach(&b.name))?;
        herdr.tag(&made.workspace, &tokens(b.phase))?;
        if matches!(b.phase, Phase::Live(_)) {
            herdr.report_agent(&made.pane, b.phase.agent_state())?;
        }
        report.push(format!("{label}: opened"));
    }
    let gone = spaces
        .iter()
        .filter(|s| s.label.starts_with(BOX_MARK) && !boxes.iter().any(|b| b.label() == s.label));
    for space in gone {
        if settle(herdr, space, Phase::Closed)? {
            report.push(format!("{}: closed", space.label));
        }
    }
    Ok(report)
}

/// `s` quoted for a POSIX shell.
#[must_use]
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;
