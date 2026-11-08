//! The `--via` command of each machine moor-hc adds. It names the controller and the Box, so
//! it is also how sync tells its own machines from the ones the person added.

use std::path::Path;

/// The program name every via command starts with.
pub const PROGRAM: &str = "moor-hc";

/// Where a via command dials the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A moor server, `https://<host>`.
    Server(String),
    /// A controller's Unix socket on this computer.
    Socket(String),
}

impl Link {
    /// The flag and value that name this link on a `moor-hc` command line.
    #[must_use]
    pub fn flag(&self) -> (&'static str, &str) {
        match self {
            Self::Server(s) => ("--server", s),
            Self::Socket(s) => ("--socket", s),
        }
    }
}

/// `moor-hc exec --server URL BOX --`: hc appends the shell line to run in the Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    /// The `moor-hc` binary, by absolute path.
    pub exe: String,
    /// The controller.
    pub link: Link,
    /// The Box.
    pub box_name: String,
}

impl Via {
    /// The command as hc stores it.
    #[must_use]
    pub fn render(&self) -> String {
        let (flag, value) = self.link.flag();
        format!(
            "{} exec {flag} {} {} --",
            quote(&self.exe),
            quote(value),
            quote(&self.box_name)
        )
    }

    /// The via command `target`, when moor-hc wrote it.
    #[must_use]
    pub fn parse(target: &str) -> Option<Self> {
        let words = words(target)?;
        let [exe, exec, flag, value, box_name, end] = words.as_slice() else {
            return None;
        };
        let ours = Path::new(exe).file_name().is_some_and(|n| n == PROGRAM);
        if !ours || exec != "exec" || end != "--" {
            return None;
        }
        let link = match flag.as_str() {
            "--server" => Link::Server(value.clone()),
            "--socket" => Link::Socket(value.clone()),
            _ => return None,
        };
        Some(Self {
            exe: exe.clone(),
            link,
            box_name: box_name.clone(),
        })
    }
}

/// `s` quoted for a POSIX shell.
#[must_use]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The words a POSIX shell splits `line` into, for lines without expansions; `None` when a
/// quote is left open.
fn words(line: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => out.extend(word.take()),
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        q => w.push(q),
                    }
                }
            }
            '"' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            e @ ('"' | '\\' | '$' | '`') => w.push(e),
                            e => w.extend(['\\', e]),
                        },
                        q => w.push(q),
                    }
                }
            }
            '\\' => word.get_or_insert_with(String::new).push(chars.next()?),
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(word);
    Some(out)
}

#[cfg(test)]
mod tests;
