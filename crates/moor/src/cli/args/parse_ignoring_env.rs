//! Parsing for tests, without the process environment's defaults.

use std::ffi::OsString;

use super::{Action, Cli};
use crate::cli::GlobalOpts;

/// `parse` with the `MOOR_SESSION` default of `--parent` ignored, so tests do not
/// depend on the process environment.
pub(in crate::cli) fn parse_ignoring_env(
    args: Vec<OsString>,
) -> Result<(GlobalOpts, Action), clap::Error> {
    use clap::{CommandFactory, FromArgMatches};
    fn strip(cmd: clap::Command) -> clap::Command {
        cmd.mut_args(|a| {
            if a.get_id() == "parent" {
                a.env(None::<&str>)
            } else {
                a
            }
        })
        .mut_subcommands(strip)
    }
    let matches = strip(Cli::command()).try_get_matches_from(args)?;
    let cli = Cli::from_arg_matches(&matches)?;
    Ok((cli.flags.into(), cli.command.into_action()))
}
