//! What a run for a pull request adds: the head it reads from GitHub as the operator's App, and
//! the variables and first line its Box gets.

use moor_core::config::with_env;
use moor_core::workflow::{CommitSha, PullRef, PullTarget};
use moor_core::ErrorKind;

use crate::adapters::github::app::AppClient;
use crate::cx::Cx;
use crate::error::CommandError;

/// The head commit of pull request `target`, read as the App the controller config names. No
/// App is a refusal: moor never reads a pull request with the owner's token.
pub(super) fn read_head(cx: &Cx, target: &PullTarget) -> Result<CommitSha, CommandError> {
    let Some(app) = cx.env().shared::<AppClient>() else {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "no GitHub App configured: a run for a pull request reads its head as the App",
        )
        .with_hint("set github_app in the controller config"));
    };
    let failed = |why: String| CommandError::new(ErrorKind::Failed, why);
    let sha = app
        .head_of(&target.repo, target.number, cx.clock().now())
        .map_err(failed)?;
    CommitSha::parse(&sha).map_err(|e| failed(format!("GitHub answered {target}: {e}")))
}

/// The override and prompt a run's Box starts with. A run for `pull` adds `MOOR_PULL_*` to the
/// override's `env` and opens the prompt with a line naming the pull request.
pub(super) fn run_input(
    pull: Option<&PullRef>,
    config: Option<String>,
    prompt: String,
) -> Result<(Option<String>, String), String> {
    let Some(pull) = pull else {
        return Ok((config, prompt));
    };
    let config = with_env(config.as_deref(), &pull.env()).map_err(|e| e.to_string())?;
    let line = format!(
        "This run is for pull request {}#{} at head {} ({}).",
        pull.repo,
        pull.number,
        pull.head_sha,
        pull.url()
    );
    Ok((Some(config), format!("{line}\n\n{prompt}")))
}
