//! The two secrets a first Box needs, read from the environment or a prompt when missing.

use std::io::{BufRead, IsTerminal, Write};

use moor_core::request::{AsReq, SecretSetReq, SecretValue};
use moor_core::user::SecretName;
use moor_core::view::user::SecretNamesValue;
use moor_core::Request;

use crate::doctor::{answer, Ask, Check};
use crate::env::Env;

/// A secret `up` sets, where its value comes from, and how to get one.
pub(crate) struct Wanted {
    /// The secret's name.
    pub name: &'static str,
    /// The variable that holds the value for a run without a prompt.
    pub var: &'static str,
    /// The command that prints a value.
    pub source: &'static str,
}

/// The secrets `up` sets, in order.
pub(crate) const WANTED: [Wanted; 2] = [
    Wanted {
        name: "claude",
        var: "CLAUDE_CODE_OAUTH_TOKEN",
        source: "claude setup-token",
    },
    Wanted {
        name: "github",
        var: "GH_TOKEN",
        source: "gh auth token",
    },
];

/// Sets each of [`WANTED`] the user lacks, from its variable, else a line of `input`.
pub(crate) fn converge(
    env: &Env,
    ask: crate::doctor::Ask<'_>,
    input: &mut dyn BufRead,
    prompt: &mut dyn Write,
) -> Vec<Result<Check, Check>> {
    let have = match answer::<SecretNamesValue>(ask, Request::SecretNames(AsReq { identity: None }))
    {
        Ok(v) => v.names,
        Err(p) => return vec![Err(Check::fail("secret", p.detail, p.fix))],
    };
    WANTED
        .iter()
        .map(|w| {
            let name = SecretName::parse(w.name).map_err(|e| Check::fail("secret", e, ""))?;
            if have.contains(&name) {
                return Ok(Check::ok("secret", format!("{}: set", w.name)));
            }
            let value = env
                .get(w.var)
                .map(str::to_owned)
                .filter(|v| !v.trim().is_empty())
                .or_else(|| read(w, input, prompt))
                .ok_or_else(|| {
                    Check::fail(
                        "secret",
                        format!("{}: not set", w.name),
                        format!(
                            "rerun moor up and paste what `{}` prints, or set {}",
                            w.source, w.var
                        ),
                    )
                })?;
            set(ask, name, value.trim())?;
            Ok(Check::ok("secret", format!("{}: set", w.name)))
        })
        .collect()
}

/// A non-empty line of `input` after a prompt for `w`; echo is off on a terminal.
fn read(w: &Wanted, input: &mut dyn BufRead, prompt: &mut dyn Write) -> Option<String> {
    let _ = write!(prompt, "{}: paste what `{}` prints: ", w.name, w.source);
    let _ = prompt.flush();
    let echo = Echo::off();
    let mut line = String::new();
    let read = input.read_line(&mut line);
    drop(echo);
    let _ = writeln!(prompt);
    read.ok()?;
    let line = line.trim().to_owned();
    (!line.is_empty()).then_some(line)
}

/// Stores `value` as secret `name` of the caller.
fn set(ask: Ask<'_>, name: SecretName, value: &str) -> Result<(), Check> {
    let req = Request::SecretSet(SecretSetReq {
        identity: None,
        name,
        value: SecretValue::new(value.as_bytes().to_vec()),
    });
    answer::<serde_json::Value>(ask, req)
        .map(drop)
        .map_err(|p| Check::fail("secret", p.detail, p.fix))
}

/// The terminal's settings while echo is off; restored on drop.
struct Echo(Option<rustix::termios::Termios>);

impl Echo {
    /// Turns echo off on stdin when it is a terminal.
    fn off() -> Self {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Self(None);
        }
        let Ok(saved) = rustix::termios::tcgetattr(&stdin) else {
            return Self(None);
        };
        let mut quiet = saved.clone();
        quiet.local_modes.remove(rustix::termios::LocalModes::ECHO);
        let set = rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &quiet);
        Self(set.is_ok().then_some(saved))
    }
}

impl Drop for Echo {
    fn drop(&mut self) {
        if let Some(saved) = &self.0 {
            let _ = rustix::termios::tcsetattr(
                std::io::stdin(),
                rustix::termios::OptionalActions::Now,
                saved,
            );
        }
    }
}
