//! Sandbox denials on a macOS Box: nono's JSON report, and the Seatbelt log read on the Box.

use super::quote;

#[cfg(test)]
mod tests;

/// Where setup's sandbox report lands, relative to the workdir.
pub const SETUP_REPORT_REL: &str = ".moor/setup.sandbox.json";
/// Where setup writes its start time (Box local time, `%Y-%m-%d %H:%M:%S`) on line 1 and its shell's pid on line 2, relative to the workdir.
pub const SETUP_START_REL: &str = ".moor/setup.sandbox.start";
/// Where the setup's sandbox denials are saved on the Box, relative to the workdir.
pub const SETUP_DENIALS_REL: &str = ".moor/setup.denials";
/// The most denials reported at once.
pub const DENIAL_LIMIT: usize = 20;
/// The longest denial line shown, in characters.
pub const DENIAL_LINE_CHARS: usize = 300;
/// The most characters kept of a parsed target, so a giant line costs nothing downstream.
const TARGET_KEEP_CHARS: usize = 4 * DENIAL_LINE_CHARS;

/// Line that starts the report section of the script's output.
const REPORT_MARKER: &str = "moor-denials: report";
/// Line that starts the log section of the script's output.
const LOG_MARKER: &str = "moor-denials: log";

/// One denied operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The process that was denied, when the log names it.
    pub process: Option<String>,
    /// The Seatbelt operation, such as `file-read-data`.
    pub operation: String,
    /// The path or name the operation aimed at.
    pub target: String,
}

impl std::fmt::Display for Denial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(process) = &self.process {
            write!(f, "{process}: ")?;
        }
        write!(f, "{} {}", self.operation, self.target)
    }
}

/// Denials split by where they came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Split {
    /// From the setup's saved time window (`.moor/setup.denials`, `.moor/setup.sandbox.json`); may include other processes.
    pub setup: Vec<Denial>,
    /// From the live log lines of the Task's own sandbox processes.
    pub live: Vec<Denial>,
}

/// What nono's stderr held: the denials in its JSON report, and any text before it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The denials in the JSON report.
    pub denials: Vec<Denial>,
    /// The text before the report, trimmed.
    pub text: String,
}

/// Splits nono's stderr report: the trailing JSON object, and any text before it (nono's own errors, trimmed).
#[must_use]
pub fn from_report(stderr: &str) -> Report {
    let starts = std::iter::once(0)
        .chain(stderr.match_indices("\n{").map(|(i, _)| i + 1))
        .filter_map(|i| stderr.split_at_checked(i));
    for (before, json) in starts {
        if !json.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
            let mut denials = report_denials(&value);
            denials.truncate(DENIAL_LIMIT);
            return Report {
                denials,
                text: before.trim().to_owned(),
            };
        }
    }
    Report {
        denials: Vec::new(),
        text: stderr.trim().to_owned(),
    }
}

/// The denials in a parsed nono report: violations first, then path denials they do not cover.
fn report_denials(value: &serde_json::Value) -> Vec<Denial> {
    let session = &value["session"];
    let strs = |v: &serde_json::Value, key: &str| v[key].as_str().map(str::to_owned);
    let mut out: Vec<Denial> = Vec::new();
    let mut push = |denial: Denial| {
        if !out.contains(&denial) {
            out.push(denial);
        }
    };
    let list = |key: &str| session[key].as_array().cloned().unwrap_or_default();
    let violations = list("violations");
    for v in &violations {
        if let (Some(operation), Some(target)) = (strs(v, "operation"), strs(v, "target")) {
            push(Denial {
                process: None,
                operation,
                target: bound(&target),
            });
        }
    }
    for d in &list("denials") {
        let (Some(path), Some(access)) = (strs(d, "path"), strs(d, "access")) else {
            continue;
        };
        if violations
            .iter()
            .any(|v| strs(v, "target").as_deref() == Some(&path))
        {
            continue;
        }
        push(Denial {
            process: None,
            operation: format!("file-{}", access.to_lowercase()),
            target: bound(&path),
        });
    }
    out
}

/// The POSIX sh script that prints the Task's recent sandbox denials on a macOS Box.
///
/// It prints the setup report, then the Seatbelt denials of the live `nono` processes whose
/// command line holds `root`, and of their descendants. It never prints environment contents.
#[must_use]
pub fn script(root: &str, workdir: &str, minutes: u32) -> String {
    let root = quote(root);
    let report = quote(&format!("{workdir}/{SETUP_REPORT_REL}"));
    let saved = quote(&format!("{workdir}/{SETUP_DENIALS_REL}"));
    format!(
        r#"echo '{REPORT_MARKER}'
if [ -f {report} ]; then cat {report}; fi
if [ -f {saved} ]; then cat {saved}; fi
echo '{LOG_MARKER}'
pids=$(ps -axo pid=,ppid=,command= | MOOR_ROOT={root} awk '
{{ pid[NR] = $1; par[NR] = $2; c = $0; sub(/^ *[0-9]+ +[0-9]+ +/, "", c); first = c; sub(/ .*/, "", first)
  n = split(first, parts, "/")
  if (parts[n] ~ /^nono/ && index(c, ENVIRON["MOOR_ROOT"]) > 0) sel[$1] = 1 }}
END {{ do {{ ch = 0
    for (i = 1; i <= NR; i++) if (!(pid[i] in sel) && (par[i] in sel)) {{ sel[pid[i]] = 1; ch = 1 }}
  }} while (ch)
  for (k in sel) print k }}')
if [ -n "$pids" ]; then
  /usr/bin/log show --last {minutes}m --style compact --predicate 'subsystem == "com.apple.sandbox.reporting"' 2>/dev/null |
    awk -v pids="$pids" '
BEGIN {{ n = split(pids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 }}
match($0, /Sandbox: [^ ]+\([0-9]+\) deny\(/) {{
  s = substr($0, RSTART, RLENGTH); sub(/\) deny\($/, "", s); sub(/^.*\(/, "", s)
  if (s in want) print }}' | tail -n 200
fi
exit 0
"#
    )
}

/// The awk program that keeps Seatbelt lines whose pid is at least `minpid`; with `minpid` 0 it keeps all.
pub const PID_FILTER_AWK: &str = r#"{ if (minpid > 0) {
  if (!match($0, /Sandbox: [^ ]+\([0-9]+\) deny\(/)) next
  s = substr($0, RSTART, RLENGTH); sub(/\) deny\($/, "", s); sub(/^.*\(/, "", s)
  if (s + 0 < minpid) next }
  print }"#;

/// The POSIX sh script, run on the macOS Box after a failed setup, that prints the Seatbelt denial
/// lines logged since setup started and saves them to `SETUP_DENIALS_REL`.
///
/// Line 1 of the start file is the start time; line 2, when all digits, is the setup shell's pid,
/// and denials of lower pids (older processes) are dropped. It prints nothing and exits 0 when the
/// start file is missing.
#[must_use]
pub fn window_script(workdir: &str) -> String {
    let start = quote(&format!("{workdir}/{SETUP_START_REL}"));
    let saved = quote(&format!("{workdir}/{SETUP_DENIALS_REL}"));
    format!(
        r#"s=$(sed -n 1p {start} 2>/dev/null) || exit 0; [ -n "$s" ] || exit 0
p=$(sed -n 2p {start} 2>/dev/null)
case "$p" in (''|*[!0-9]*) p=0 ;; esac
/usr/bin/log show --start "$s" --style compact --predicate 'subsystem == "com.apple.sandbox.reporting"' 2>/dev/null | grep ' deny(' | awk -v minpid="$p" '{PID_FILTER_AWK}' | tail -n 500 > {saved}; cat {saved}; exit 0
"#
    )
}

/// The denials the script printed: setup's, then the live ones; deduplicated, oldest first, at most `DENIAL_LIMIT`.
///
/// Raw Seatbelt log lines count with or without the section markers, also inside the report
/// section. When too many remain, blocking operations (file, network, exec, forbidden) win over noise.
#[must_use]
pub fn parse(stdout: &str) -> Vec<Denial> {
    let split = split_all(stdout);
    cap_denials(merge(split.setup.into_iter().chain(split.live)))
}

/// The script output of [`script`] split by section; each list deduplicated and capped like [`parse`].
#[must_use]
pub fn parse_split(stdout: &str) -> Split {
    let split = split_all(stdout);
    Split {
        setup: cap_denials(merge(split.setup)),
        live: cap_denials(merge(split.live)),
    }
}

/// Both lists of [`parse_split`], before deduplication and cap.
fn split_all(stdout: &str) -> Split {
    let mut report = String::new();
    let mut setup_logged: Vec<Denial> = Vec::new();
    let mut live: Vec<Denial> = Vec::new();
    let mut in_report = false;
    for line in stdout.lines() {
        match line.trim_end() {
            REPORT_MARKER => in_report = true,
            LOG_MARKER => in_report = false,
            _ => {
                if let Some(denial) = log_line(line) {
                    if in_report {
                        setup_logged.push(denial);
                    } else {
                        live.push(denial);
                    }
                } else if in_report {
                    report.push_str(line);
                    report.push('\n');
                }
            }
        }
    }
    let from_json = if report.trim_start().starts_with('{') {
        from_report(&report).denials
    } else {
        Vec::new()
    };
    Split {
        setup: from_json.into_iter().chain(setup_logged).collect(),
        live,
    }
}

/// Deduplicates, keeping the latest of each.
fn merge(denials: impl IntoIterator<Item = Denial>) -> Vec<Denial> {
    let mut merged: Vec<Denial> = Vec::new();
    for denial in denials {
        merged.retain(|d| *d != denial);
        merged.push(denial);
    }
    merged
}

/// `text` cut to `TARGET_KEEP_CHARS` characters.
fn bound(text: &str) -> String {
    text.chars().take(TARGET_KEEP_CHARS).collect()
}

/// `text` with ANSI escape sequences (CSI and OSC) and other control characters removed (tab and newline become a space) and whitespace runs collapsed; no length cap.
#[must_use]
pub fn strip(text: &str) -> String {
    let mut kept = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\u{1b}', Some('[')) => {
                chars.next();
                while chars
                    .next_if(|p| ('\u{20}'..='\u{3f}').contains(p))
                    .is_some()
                {}
                chars.next_if(|p| ('\u{40}'..='\u{7e}').contains(p));
            }
            ('\u{1b}', Some(']')) => {
                chars.next();
                while let Some(o) = chars.next() {
                    if o == '\u{7}' || (o == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            ('\t' | '\n', _) => kept.push(' '),
            _ if !c.is_control() => kept.push(c),
            _ => {}
        }
    }
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to `DENIAL_LINE_CHARS` characters, ending in `…` when cut.
#[must_use]
pub fn cap(text: &str) -> String {
    if text.chars().count() <= DENIAL_LINE_CHARS {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(DENIAL_LINE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// `text` safe to show when it needs no redaction: `cap(&strip(text))`.
#[must_use]
pub fn clean(text: &str) -> String {
    cap(&strip(text))
}

/// Whether the operation can block a command, as opposed to preference or mach lookups.
fn is_blocking(operation: &str) -> bool {
    ["file-", "network-", "process-exec", "forbidden-"]
        .iter()
        .any(|p| operation.starts_with(p))
}

/// Keeps at most `DENIAL_LIMIT`: the latest blocking ones first, then the latest others; oldest first.
fn cap_denials(denials: Vec<Denial>) -> Vec<Denial> {
    if denials.len() <= DENIAL_LIMIT {
        return denials;
    }
    let mut left = DENIAL_LIMIT;
    let mut chosen = vec![false; denials.len()];
    for blocking in [true, false] {
        for (picked, d) in chosen.iter_mut().zip(&denials).rev() {
            if left > 0 && is_blocking(&d.operation) == blocking {
                *picked = true;
                left -= 1;
            }
        }
    }
    denials
        .into_iter()
        .zip(chosen)
        .filter_map(|(d, k)| k.then_some(d))
        .collect()
}

/// The denial in one Seatbelt log line, if it is one.
fn log_line(line: &str) -> Option<Denial> {
    let (_, rest) = line.split_once("Sandbox: ")?;
    let (process, rest) = rest.split_once('(')?;
    let (_, rest) = rest.split_once(") deny(")?;
    let (_, rest) = rest.split_once(") ")?;
    let (operation, target) = rest.trim().split_once(' ')?;
    Some(Denial {
        process: Some(process.to_owned()),
        operation: operation.to_owned(),
        target: bound(target.trim()),
    })
}
