//! Recovery of global options and the verb from argv that failed to parse.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Args, CommandFactory, FromArgMatches, Parser};

use super::GlobalOpts;

/// Recovers the global options and the verb from argv that failed to parse.
///
/// Uses the same `Flags` as `Cli`, ignoring every other error.
pub(super) fn recover(args: &[OsString]) -> (GlobalOpts, String) {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let args = args.get(..end).unwrap_or(args);
    let mut args = args.to_vec();
    let probe = loop {
        match Probe::command().try_get_matches_from(&args) {
            Ok(m) => break Probe::from_arg_matches(&m).ok(),
            Err(e) => match unknown_arg(&e).and_then(|u| args.iter().position(|a| starts(a, &u))) {
                Some(i) => {
                    args.remove(i);
                }
                None => break None,
            },
        }
    };
    let Some(probe) = probe else {
        return (GlobalOpts::default(), String::new());
    };
    let cmd = probe
        .rest
        .into_iter()
        .find(|a| !a.starts_with('-'))
        .unwrap_or_default();
    (probe.flags.into(), cmd)
}

/// The argument clap rejected as unknown, if that is the error.
fn unknown_arg(e: &clap::Error) -> Option<String> {
    match (e.kind(), e.get(ContextKind::InvalidArg)) {
        (ErrorKind::UnknownArgument, Some(ContextValue::String(s))) => Some(s.clone()),
        _ => None,
    }
}

/// Whether `arg` is, or is an `=` / short-cluster form of, `flag`.
fn starts(arg: &OsString, flag: &str) -> bool {
    arg.to_str().is_some_and(|a| a.starts_with(flag))
}

/// Global flags shared by `Cli` and `Probe`.
#[derive(Debug, Args)]
pub(super) struct Flags {
    /// Print one JSON envelope on stdout.
    #[arg(long, global = true)]
    pub(super) json: bool,
    /// Suppress warnings in text mode.
    #[arg(long, global = true)]
    pub(super) quiet: bool,
    /// Directory holding moor state.
    #[arg(long, global = true, value_name = "DIR")]
    pub(super) state_dir: Option<PathBuf>,
}

impl From<Flags> for GlobalOpts {
    fn from(f: Flags) -> Self {
        Self {
            json: f.json,
            quiet: f.quiet,
            state_dir: f.state_dir,
        }
    }
}

/// Globals-only command line used to recover options after a parse error.
#[derive(Debug, Parser)]
#[command(name = "moor", disable_help_flag = true, disable_version_flag = true)]
struct Probe {
    /// Global flags.
    #[command(flatten)]
    flags: Flags,
    /// Positionals; the first is the verb.
    rest: Vec<String>,
}
