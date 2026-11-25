//! clap definitions for `moor repo`: a repo's config as the controller stores it.

use std::io::Read as _;

use clap::{Args, Subcommand};
use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq, RepoFile};
use moor_core::seam::forge::ForgeRepo;
use moor_core::Request;

/// Subcommands of `repo`.
#[derive(Debug, Subcommand)]
pub(super) enum RepoCli {
    /// A repo's config, which lives in moor, not in the repo.
    #[command(subcommand)]
    Config(ConfigCli),
    /// The Proxy rules a repo's Boxes follow, which live in moor. A repo with none stored
    /// shows the default rules text.
    #[command(subcommand)]
    Rules(ConfigCli),
}

/// Subcommands of `repo config`.
#[derive(Debug, Subcommand)]
pub(super) enum ConfigCli {
    /// Print the stored text; `--json` adds its revision and who last edited it.
    Get(RepoArg),
    /// Replace the stored text. You need push access to the repo.
    Set(SetArgs),
}

/// The repo a config belongs to.
#[derive(Debug, Args)]
pub(super) struct RepoArg {
    /// The repo, as OWNER/NAME or a github.com URL.
    #[arg(value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    repo: ForgeRepo,
}

/// Arguments of `repo config set`.
#[derive(Debug, Args)]
pub(super) struct SetArgs {
    /// The repo, as OWNER/NAME or a github.com URL.
    #[arg(value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    repo: ForgeRepo,
    /// The YAML to store, as a repo config holds it; `-` reads stdin.
    #[arg(
        long = "file",
        short = 'f',
        value_name = "FILE",
        default_value = "-",
        value_parser = config_text
    )]
    text: String,
    /// Refuse unless the stored revision is N, where 0 means no config yet, so a change made
    /// since you read it is not overwritten. `repo config get --json` shows the revision.
    #[arg(long, value_name = "N")]
    expected_revision: Option<u64>,
}

/// The text of config file `path`, or of stdin for `-`.
pub(super) fn config_text(path: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    if path == "-" {
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read stdin: {e}"))?;
    } else {
        bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    }
    String::from_utf8(bytes).map_err(|_| "the config is not UTF-8 text".to_owned())
}

impl RepoCli {
    /// Convert the parsed verb into its request.
    pub(super) fn into_request(self) -> Request {
        let (file, verb) = match self {
            Self::Config(verb) => (RepoFile::Config, verb),
            Self::Rules(verb) => (RepoFile::Rules, verb),
        };
        match verb {
            ConfigCli::Get(a) => Request::RepoConfigGet(RepoConfigGetReq {
                file,
                identity: None,
                repo: a.repo,
            }),
            ConfigCli::Set(a) => Request::RepoConfigSet(RepoConfigSetReq {
                file,
                identity: None,
                repo: a.repo,
                text: a.text,
                expected_revision: a.expected_revision,
            }),
        }
    }
}
