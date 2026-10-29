//! What crabbox says on stderr while it brings a Box up, in moor's words.
//!
//! Only known line shapes pass, rebuilt from fixed words and port numbers: a line can carry a
//! host, a lease id or anything a provider adapter printed, and none of that is forwarded.

use moor_core::seam::driver::DriverError;

/// What one acquire has reported so far.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// Every note already handed on, so a retry loop reports each one once.
    seen: Vec<String>,
    /// The latest note, repeated or not.
    latest: Option<String>,
}

impl Watch {
    /// Reads one stderr `line`; hands `note` its note when it is one not reported before.
    pub(super) fn see(&mut self, line: &str, note: &mut dyn FnMut(&str)) {
        let Some(text) = note_of(line) else {
            return;
        };
        if !self.seen.contains(&text) {
            note(&text);
            self.seen.push(text.clone());
        }
        self.latest = Some(text);
    }

    /// `err`, naming what the acquire was waiting on when it ran out of time.
    pub(super) fn explain(&self, err: DriverError) -> DriverError {
        match (err, &self.latest) {
            (DriverError::Timeout(after), Some(latest)) => DriverError::Failed {
                summary: format!("timed out after {}s {latest}", after.as_secs()),
            },
            (err, _) => err,
        }
    }
}

/// The note for a crabbox stderr `line`, or `None` for a line of no known shape.
fn note_of(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(why) = ssh_error(line) {
        return Some(format!("waiting for SSH: {why}"));
    }
    if line.starts_with("waiting for coordinator lease ") {
        return Some("waiting for the provider to hand out a machine".to_owned());
    }
    if line.starts_with("waiting for ") && line.contains(" elapsed=") {
        return Some(ssh_wait(line));
    }
    if line.starts_with("leased ") && line.ends_with(" via coordinator") {
        return Some("machine created".to_owned());
    }
    if let Some(p) = line.strip_prefix("using ssh port ") {
        let port = p.split(' ').next().filter(|p| is_port(p))?;
        return Some(format!("SSH answered on port {port}"));
    }
    if line.starts_with("warning: bootstrap failed; retrying with fresh lease") {
        return Some("machine setup failed; retrying on a new machine".to_owned());
    }
    if line.starts_with("warning: coordinator returned stale instance; retrying") {
        return Some("the provider returned a stale machine; retrying on a new one".to_owned());
    }
    if line.starts_with("syncing ") {
        return Some("syncing the workdir".to_owned());
    }
    if line.starts_with("sync complete in ") {
        return Some("workdir synced".to_owned());
    }
    None
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

/// crabbox's SSH wait progress line, told from the state of each port it probed.
fn ssh_wait(line: &str) -> String {
    let ports = line
        .split(' ')
        .find_map(|w| w.strip_prefix("ports="))
        .unwrap_or_default();
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
    if ports == "proxy" {
        "waiting for SSH through the provider's proxy".to_owned()
    } else if !auth.is_empty() {
        format!("waiting for SSH: login refused on port {auth}")
    } else if !tcp.is_empty() {
        format!("waiting for SSH: port {tcp} open but SSH does not answer")
    } else if !closed.is_empty() {
        format!("waiting for SSH: port {closed} closed")
    } else {
        "waiting for SSH".to_owned()
    }
}

/// Whether `s` is a port number.
fn is_port(s: &str) -> bool {
    s.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests;
