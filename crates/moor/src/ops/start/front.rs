//! A start a front sends: a plain Box with setup and no agent.

use moor_core::view::StartView;
use moor_core::{ErrorKind, StartReq};

use super::run;
use crate::cx::Cx;
use crate::error::CommandError;

/// Why a start from a front refuses what only an agent reads.
const AGENT_ONLY: &str = "a Box a person starts runs no agent";

/// Starts or attaches to the Box a front asks for: a plain Box that runs the repo's setup and no
/// agent. A task with words starts in an agent place through `ask`, so a start carrying a prompt,
/// an issue, a model, a resume or gates is refused before anything changes.
///
/// # Errors
/// Usage for a start carrying any of those; otherwise as [`run`].
pub(crate) fn from_front(cx: &Cx, req: StartReq) -> Result<StartView, CommandError> {
    let carried = [
        (req.prompt.is_some(), "a prompt"),
        (req.issue.is_some(), "an issue"),
        (req.model.is_some(), "a model"),
        (req.resume, "resume"),
        (req.gates.is_some(), "gates"),
    ];
    if let Some((_, what)) = carried.iter().find(|(set, _)| *set) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("a start carries no {what}: {AGENT_ONLY}"),
        )
        .with_hint(
            "start a task with `moor ask --repo OWNER/NAME TEXT` or the ask tool; its agent \
             starts work Boxes as it needs them",
        ));
    }
    run(
        cx,
        &StartReq {
            no_agent: true,
            ..req
        },
    )
}
