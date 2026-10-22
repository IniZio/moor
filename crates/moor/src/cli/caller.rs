//! The CLI resolves everything its caller's directory means before it sends a request, because
//! the controller never reads a directory of its own.

use std::path::Path;

use moor_core::name::ForgeRepo;
use moor_core::seam::checkout::Checkout;
use moor_core::{CpReq, EnvReq, Request, StartReq, Warning};

/// The request to send, with any warning the CLI adds to its reply.
#[derive(Debug)]
pub struct Resolved {
    /// The request with the caller's repo filled in.
    pub request: Request,
    /// A warning about the caller's checkout.
    pub warning: Option<Warning>,
}

/// Refusal for a `--repo` value; the value is never quoted, as it may carry a token.
pub(super) const REPO_ARG: &str = "--repo takes OWNER/NAME or a github.com repo URL";

/// Refusal for an origin moor cannot clone; the URL is never quoted, as it may carry a token.
const NOT_GITHUB: &str =
    "the origin remote is not a github.com repo; name one with --repo OWNER/NAME";

/// `request` resolved against `cwd`, reading the checkout there through `git`.
///
/// # Errors
/// The refusal text for a checkout whose origin is not on github.com.
pub fn resolve(request: Request, cwd: &Path, git: &dyn Checkout) -> Result<Resolved, String> {
    let repo = || caller_repo(cwd, git);
    let plain = |request| Resolved {
        request,
        warning: None,
    };
    Ok(match request {
        Request::Start(r) if r.repo.is_none() => {
            let repo = repo()?;
            let warning = repo
                .as_ref()
                .filter(|_| git.local_changes(cwd))
                .map(local_changes);
            Resolved {
                request: Request::Start(StartReq { repo, ..r }),
                warning,
            }
        }
        Request::StartNeeds(mut r) if r.repo.is_none() => {
            r.repo = repo()?;
            plain(Request::StartNeeds(r))
        }
        Request::Env(EnvReq { repo: None, base }) => plain(Request::Env(EnvReq {
            repo: repo()?,
            base,
        })),
        Request::Cp(r) => plain(Request::Cp(CpReq {
            dest: cwd.join(r.dest).display().to_string(),
            ..r
        })),
        other => plain(other),
    })
}

/// The github.com repo the checkout at `cwd` pushes to; `None` outside a checkout or without
/// an origin, so the Box starts with an empty workdir.
fn caller_repo(cwd: &Path, git: &dyn Checkout) -> Result<Option<ForgeRepo>, String> {
    git.origin_url(cwd)
        .map(|url| ForgeRepo::from_github_url(&url).ok_or_else(|| NOT_GITHUB.to_owned()))
        .transpose()
}

/// The warning that local work stays out of the Box, which starts from what `repo` has pushed.
fn local_changes(repo: &ForgeRepo) -> Warning {
    Warning {
        code: "local_changes_not_in_box".to_owned(),
        message: format!(
            "this checkout has uncommitted or unpushed changes; the Box starts from what is pushed to {repo}"
        ),
    }
}

/// Parses `--repo`: `OWNER/NAME`, or a github.com URL in any form an origin takes.
///
/// # Errors
/// The usage text for anything else.
pub(super) fn repo_arg(s: &str) -> Result<ForgeRepo, String> {
    ForgeRepo::parse(s)
        .ok()
        .or_else(|| ForgeRepo::from_github_url(s))
        .ok_or_else(|| REPO_ARG.to_owned())
}

#[cfg(test)]
mod tests;
