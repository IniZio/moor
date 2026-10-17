//! Reads what `docker compose up` and `docker ps` print.

/// What a failed `up` says went wrong.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Clash {
    /// A published host port is taken.
    Port(String),
    /// A container name is taken.
    Name {
        /// The container name, without its leading slash.
        name: String,
        /// The container that has it, when the error names one.
        holder: Option<String>,
    },
    /// Anything else.
    Other,
}

/// Sorts the combined output of a failed `up` into a clash or `Other`.
pub(super) fn classify(log: &str) -> Clash {
    for line in log.lines() {
        if line.contains("port is already allocated") || line.contains("address already in use") {
            if let Some(port) = port_of(line) {
                return Clash::Port(port);
            }
        }
        if let Some(rest) = line.split("container name \"").nth(1) {
            if let Some((name, after)) = rest.split_once('"') {
                let holder = after
                    .split("by container \"")
                    .nth(1)
                    .and_then(|h| h.split_once('"'))
                    .map(|(h, _)| h.to_owned());
                return Clash::Name {
                    name: name.trim_start_matches('/').to_owned(),
                    holder,
                };
            }
        }
    }
    Clash::Other
}

/// The first `host:port` token of the line, as the port.
fn port_of(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|t| {
        let port = t.trim_end_matches(':').rsplit_once(':')?.1;
        (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then(|| port.to_owned())
    })
}

/// Prefix of each `docker ps` line that names a container; the Driver's own banner lines share the log.
pub(super) const HOLDER_MARK: &str = "moor-holder\t";

/// The first marked container of `docker ps` output as name and compose project.
pub(super) fn holder(out: &str) -> Option<(String, Option<String>)> {
    let line = out.lines().find_map(|l| l.strip_prefix(HOLDER_MARK))?;
    let (name, project) = line.split_once('\t').unwrap_or((line, ""));
    let project = project.trim();
    Some((
        name.trim().to_owned(),
        (!project.is_empty()).then(|| project.to_owned()),
    ))
}

/// Marker line before the framed command output.
pub(super) const BEGIN: &str = "moor-up-begin";

/// Marker prefix of the line after the framed command output; the exit status follows.
pub(super) const END: &str = "moor-up-end";

/// What the command itself printed: the lines between the begin and end markers.
///
/// The Driver log also holds its own banner before and its run summary after, which are never
/// read. `None` when either marker is missing.
pub(super) fn framed(log: &str) -> Option<String> {
    let mut lines = log.lines().skip_while(|l| *l != BEGIN).skip(1);
    let mut out = String::new();
    for l in lines.by_ref() {
        if l.strip_prefix(END).is_some_and(|r| r.starts_with(' ')) {
            return Some(out);
        }
        out.push_str(l);
        out.push('\n');
    }
    None
}

/// The last non-blank line of `out`.
pub(super) fn last_line(out: &str) -> Option<&str> {
    out.lines().rev().find(|l| !l.trim().is_empty())
}
