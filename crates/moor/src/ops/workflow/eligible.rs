//! Whether a pull request event fires, decided on the pull request as GitHub has it now.
//!
//! Deliveries arrive out of order, so a payload only names the pull request to read. The App
//! reads it back: its state, draft flag, head, author and the repo its head lives in. A closed
//! pull request cancels. A draft or a bot's pull request records nothing. A fork fires only when
//! its author has admin, maintain or write on the base repo, which is read only for a fork that
//! would otherwise fire.

use moor_core::name::ForgeRepo;
use moor_core::workflow::{CommitSha, PullTarget};

use crate::adapters::github::app::pull::{Permission, Pull};
use crate::adapters::github::app::AppClient;
use crate::cx::Cx;

/// What the pull request as GitHub has it asks of the workflows on its repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    /// It is closed, merged or not: cancel its open runs.
    Closed,
    /// It is a draft or a bot's: record nothing, for this reason.
    Skip(&'static str),
    /// It comes from a fork whose author cannot write to the base repo: record `skipped_fork`.
    Fork {
        /// Its head commit.
        head_sha: CommitSha,
        /// Its author's login.
        author: String,
    },
    /// Fire at its head.
    Fire {
        /// Its head commit.
        head_sha: CommitSha,
    },
}

/// The verdict on `pull`, a pull request on `base`. `can_write` reads whether an author may
/// write to `base`; it is called only for a fork that would otherwise fire.
///
/// # Errors
/// `can_write` failed.
pub(super) fn verdict(
    pull: Pull,
    base: &ForgeRepo,
    can_write: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<Verdict, String> {
    if !pull.open {
        return Ok(Verdict::Closed);
    }
    if pull.draft {
        return Ok(Verdict::Skip("it is a draft"));
    }
    if pull.author_is_bot {
        return Ok(Verdict::Skip("a bot opened it"));
    }
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let in_base = pull
        .head_repo
        .as_ref()
        .is_some_and(|h| same(&h.owner, &base.owner) && same(&h.name, &base.name));
    Ok(if in_base || can_write(&pull.author)? {
        Verdict::Fire {
            head_sha: pull.head_sha,
        }
    } else {
        Verdict::Fork {
            head_sha: pull.head_sha,
            author: pull.author,
        }
    })
}

/// The verdict on pull request `target`, read as the App the controller config names.
///
/// # Errors
/// No App is configured, it is not installed on the repo, or GitHub cannot be reached or refuses
/// a read.
pub(super) fn read_back(cx: &Cx, target: &PullTarget) -> Result<Verdict, String> {
    let Some(app) = cx.env().shared::<AppClient>() else {
        return Err("no GitHub App configured to read the pull request with".to_owned());
    };
    let token = app.token_for(&target.repo, cx.clock().now())?;
    let pull = app.pull(&target.repo, target.number, &token)?;
    verdict(pull, &target.repo, |author| {
        app.permission(&target.repo, author, &token)
            .map(Permission::can_write)
    })
}
