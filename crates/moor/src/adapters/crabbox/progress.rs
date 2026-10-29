//! What crabbox says on stderr while it brings a Box up, in moor's words.
//!
//! Only known line shapes pass, rebuilt from fixed words and port numbers: a line can carry a
//! host, a lease id or anything a provider adapter printed, and none of that is forwarded.

use moor_core::seam::driver::DriverError;

use crate::adapters::proc::plain::last_sentence;

/// Longest gap, in seconds of crabbox's own wait clock, before an unchanged wait is reported again.
const WAIT_TICK: u64 = 60;

/// What one stderr line says.
#[derive(Debug, PartialEq)]
enum Said {
    /// A step of the acquire, reported once.
    Step(String),
    /// Still waiting for SSH, with what is in the way when known.
    Wait {
        /// ssh's complaint or the port states, in fixed words.
        cause: Option<String>,
        /// Seconds crabbox has waited so far.
        elapsed: Option<u64>,
    },
    /// crabbox gave up waiting for SSH, with ssh's last complaint when it named one.
    GaveUp(Option<String>),
}

/// The SSH wait last reported.
#[derive(Debug)]
struct Waited {
    /// What was in the way.
    cause: Option<String>,
    /// crabbox's elapsed seconds when it was reported.
    at: u64,
}

/// What one acquire has reported so far.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// Every step already handed on, so a retry loop reports each one once.
    seen: Vec<String>,
    /// The SSH wait last handed on.
    waited: Option<Waited>,
    /// The latest note, repeated or not.
    latest: Option<String>,
    /// The failure crabbox named when it stopped waiting for SSH.
    gave_up: Option<String>,
}

impl Watch {
    /// Reads one stderr `line`; hands `note` its note when it says something not reported yet.
    pub(super) fn see(&mut self, line: &str, note: &mut dyn FnMut(&str)) {
        match said(line) {
            None => {}
            Some(Said::Step(text)) => {
                if !self.seen.contains(&text) {
                    note(&text);
                    self.seen.push(text.clone());
                }
                self.latest = Some(text);
            }
            Some(Said::Wait { cause, elapsed }) => {
                let text = cause.as_ref().map_or_else(
                    || {
                        elapsed.map_or_else(
                            || "waiting for SSH".to_owned(),
                            |e| format!("waiting for SSH ({})", clock(e)),
                        )
                    },
                    |c| format!("waiting for SSH: {c}"),
                );
                let at = elapsed.unwrap_or_default();
                let due = self.waited.as_ref().is_none_or(|w| {
                    w.cause != cause || (cause.is_none() && at >= w.at + WAIT_TICK)
                });
                if due {
                    note(&text);
                    self.waited = Some(Waited { cause, at });
                }
                self.latest = Some(text);
            }
            Some(Said::GaveUp(cause)) => {
                self.gave_up = Some(cause.map_or_else(
                    || "timed out waiting for SSH".to_owned(),
                    |c| format!("timed out waiting for SSH: {c}"),
                ));
            }
        }
    }

    /// `err`, naming what the acquire was waiting on when it ran out of time.
    pub(super) fn explain(&self, err: DriverError) -> DriverError {
        match (err, &self.gave_up, &self.latest) {
            (DriverError::Timeout(_) | DriverError::Failed { .. }, Some(gave_up), _) => {
                DriverError::Failed {
                    summary: gave_up.clone(),
                }
            }
            (DriverError::Timeout(after), None, Some(latest)) => DriverError::Failed {
                summary: format!("timed out after {}s {latest}", after.as_secs()),
            },
            (err, ..) => err,
        }
    }
}

/// What a crabbox stderr `line` says, or `None` for a line of no known shape.
fn said(line: &str) -> Option<Said> {
    let line = line.trim();
    if let Some(rest) = line.split("timed out waiting for SSH on ").nth(1) {
        let complaint = rest
            .split_once(" during ")
            .and_then(|(_, r)| r.split_once(": "))
            .map(|(_, r)| r.split("; next_action=").next().unwrap_or(r));
        return Some(Said::GaveUp(complaint.and_then(cause_of)));
    }
    if let Some(why) = ssh_error(line) {
        return Some(Said::Wait {
            cause: Some(why),
            elapsed: None,
        });
    }
    if line.starts_with("waiting for coordinator lease ") {
        return Some(step("waiting for the provider to hand out a machine"));
    }
    if line.starts_with("waiting for ") && line.contains(" elapsed=") {
        return Some(ssh_wait(line));
    }
    if let Some(rest) = line.strip_prefix("acquire: ") {
        return Some(Said::Step(created(rest).or_else(|| last_sentence(rest))?));
    }
    if let Some(rest) = line.strip_prefix("tools: ") {
        return Some(Said::Step(last_sentence(rest)?));
    }
    note_of(line)
}

/// A step that says `text`.
fn step(text: &str) -> Said {
    Said::Step(text.to_owned())
}

/// The note for one of crabbox's fixed-wording lines.
fn note_of(line: &str) -> Option<Said> {
    if line.starts_with("leased ") && line.ends_with(" via coordinator") {
        return Some(step("machine created"));
    }
    if let Some(p) = line.strip_prefix("using ssh port ") {
        let port = p.split(' ').next().filter(|p| is_port(p))?;
        return Some(Said::Step(format!("SSH answered on port {port}")));
    }
    [
        (
            "warning: bootstrap failed; retrying with fresh lease",
            "machine setup failed; retrying on a new machine",
        ),
        (
            "warning: coordinator returned stale instance; retrying",
            "the provider returned a stale machine; retrying on a new one",
        ),
        ("syncing ", "syncing the workdir"),
        ("sync complete in ", "workdir synced"),
    ]
    .into_iter()
    .find(|(shape, _)| line.starts_with(shape))
    .map(|(_, words)| step(words))
}

/// `acquire: cold; created <id> with a <n> GB disk`, without the id.
fn created(rest: &str) -> Option<String> {
    let gb = rest
        .split_once("created ")?
        .1
        .split_once(" with a ")?
        .1
        .strip_suffix(" GB disk")
        .filter(|n| n.parse::<u32>().is_ok())?;
    Some(format!("machine created with a {gb} GB disk"))
}

/// ssh's complaint `text` in words that cannot carry a host or a secret.
fn cause_of(text: &str) -> Option<String> {
    ssh_error(text).or_else(|| last_sentence(text))
}

/// An ssh client error anywhere in `line`, in fixed words.
fn ssh_error(line: &str) -> Option<String> {
    if let Some(rest) = line.split("Permission denied (").nth(1) {
        let methods = rest.split(')').next().filter(|m| {
            !m.is_empty()
                && m.chars()
                    .all(|c| c.is_ascii_lowercase() || c == ',' || c == '-')
        })?;
        return Some(format!("Permission denied ({methods})"));
    }
    [
        ("Connection refused", "connection refused"),
        ("Connection timed out", "connection timed out"),
        ("No route to host", "no route to host"),
        (
            "Host key verification failed",
            "host key verification failed",
        ),
    ]
    .into_iter()
    .find(|(shape, _)| line.contains(shape))
    .map(|(_, words)| words.to_owned())
}

/// crabbox's SSH wait progress line, told from ssh's last complaint or the state of each port.
fn ssh_wait(line: &str) -> Said {
    let (probe, last) = line
        .split_once(" last=")
        .map_or((line, None), |(p, l)| (p, Some(l)));
    let field = |key: &str| probe.split(' ').find_map(|w| w.strip_prefix(key));
    let ports = field("ports=").unwrap_or_default();
    let with = |state: &str| {
        ports
            .split(',')
            .filter_map(|p| p.split_once(':'))
            .filter(|(port, s)| *s == state && is_port(port))
            .map(|(port, _)| port)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (auth, tcp, closed) = (with("auth"), with("tcp"), with("closed"));
    let cause = last.and_then(cause_of).or_else(|| {
        if !auth.is_empty() {
            Some(format!("login refused on port {auth}"))
        } else if !tcp.is_empty() {
            Some(format!("port {tcp} open but SSH does not answer"))
        } else if !closed.is_empty() {
            Some(format!("port {closed} closed"))
        } else {
            None
        }
    });
    Said::Wait {
        cause,
        elapsed: field("elapsed=").and_then(secs),
    }
}

/// Whole seconds in a Go duration such as `6s`, `2m4s` or `1h0m0s`.
fn secs(d: &str) -> Option<u64> {
    let (mut total, mut num) = (0_u64, String::new());
    for c in d.chars() {
        let unit = match c {
            '0'..='9' | '.' => {
                num.push(c);
                continue;
            }
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        let whole = num
            .split('.')
            .next()
            .unwrap_or_default()
            .parse::<u64>()
            .ok()?;
        total += whole * unit;
        num.clear();
    }
    num.is_empty().then_some(total)
}

/// `secs` the way crabbox prints a wait: `6s`, `2m4s`, `1h3m`.
fn clock(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{}s", secs / 60, secs % 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

/// Whether `s` is a port number.
fn is_port(s: &str) -> bool {
    s.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests;
