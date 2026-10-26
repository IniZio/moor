//! What a new Box's repo holds at its base, read through the forge before any machine exists.

use moor_core::config::proxy::GITHUB_SECRET;
use moor_core::seam::forge::{ForgeError, ForgeRepo, RepoAt};
use moor_core::user::{SecretName, UserId};
use moor_core::{ErrorDetail, ErrorKind, Secret, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::vault::Vault;

/// Reads `repo` at `base` as `user`'s GitHub account, with the tip of the Box `branch`.
///
/// A branch that already exists on the forge is refused unless `base` names it, so a new Box never
/// silently continues someone else's work. An account that cannot push only warns.
///
/// # Errors
/// Refused for a repo the account cannot see, a rejected or missing token, or a taken branch;
/// Usage for a base that names no commit; Failed when the forge cannot answer.
pub(crate) fn read(
    cx: &Cx,
    user: UserId,
    repo: &ForgeRepo,
    base: Option<&str>,
    branch: &str,
) -> Result<RepoAt, CommandError> {
    let at = fetch(cx, user, repo, base, branch)?;
    if at.branch_tip.is_some() && base != Some(branch) {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("branch {branch} already exists on {repo}"),
        )
        .with_hint(format!(
            "continue it with --base {branch}, or pick another Box name"
        )));
    }
    if !at.can_push {
        cx.warn(Warning {
            code: "repo_read_only".to_owned(),
            message: format!(
                "your GitHub account cannot push to {repo}; work in the Box can leave it only by a fork or moor cp"
            ),
        });
    }
    Ok(at)
}

/// Reads `repo` at `base` as `user`'s GitHub account, with no check on what it finds.
///
/// # Errors
/// As [`read`], without the branch refusal.
pub(crate) fn fetch(
    cx: &Cx,
    user: UserId,
    repo: &ForgeRepo,
    base: Option<&str>,
    branch: &str,
) -> Result<RepoAt, CommandError> {
    let token = github_token(cx, user)?;
    cx.forge()
        .read(repo, base, branch, &token)
        .map_err(|e| refusal(repo, e))
}

/// `user`'s GitHub token from the vault; it never leaves this process except to the forge.
fn github_token(cx: &Cx, user: UserId) -> Result<Secret, CommandError> {
    let name =
        SecretName::parse(GITHUB_SECRET).map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    let vault =
        Vault::load(cx.vault_key()?).map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    cx.store()?
        .user_secret(&vault, user, &name)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                format!("log in first: moor user secret set {GITHUB_SECRET}"),
            )
            .with_detail(ErrorDetail::LoginRequired)
        })
}

/// The start refusal for a failed read of `repo`; the forge error never carries the token.
fn refusal(repo: &ForgeRepo, e: ForgeError) -> CommandError {
    match e {
        ForgeError::NotVisible => CommandError::new(
            ErrorKind::Refused,
            format!("repo {repo} is not visible to your GitHub account"),
        )
        .with_hint("check OWNER/NAME, or ask the owner for access"),
        ForgeError::UnknownBase(b) => CommandError::new(
            ErrorKind::Usage,
            format!("unknown base {b}: it names no commit in {repo}"),
        ),
        ForgeError::Rejected => CommandError::new(ErrorKind::Refused, "GitHub rejected your token")
            .with_hint(format!("moor user secret set {GITHUB_SECRET}")),
        e @ (ForgeError::Unreadable(_) | ForgeError::Unavailable(_)) => CommandError::new(
            ErrorKind::Failed,
            format!("cannot read {repo} from GitHub: {e}"),
        ),
    }
}
