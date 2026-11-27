//! A start a front sends: a plain Box with setup and no agent.

use moor_core::view::StartView;
use moor_core::{ErrorKind, StartReq};

use super::run;
use crate::cx::Cx;
use crate::error::CommandError;

/// What to type instead of a start that carries what only an agent reads.
pub(crate) const ASK_INSTEAD: &str =
    "to start a task with words, type `moor ask --repo OWNER/NAME \
     TEXT` or use the ask tool; its agent starts work Boxes as it needs them";

/// Starts or attaches to the Box a front asks for: a plain Box that runs the repo's setup and no
/// agent. A task with words starts in an agent place through `ask`, so a start carrying a prompt,
/// an issue, a model, a resume or gates is refused before anything changes.
///
/// # Errors
/// Usage for a start carrying any of those; otherwise as [`run`].
pub(crate) fn from_front(cx: &Cx, req: StartReq) -> Result<StartView, CommandError> {
    let carried = [
        (req.prompt.is_some(), "prompt"),
        (req.issue.is_some(), "issue"),
        (req.model.is_some(), "model"),
        (req.resume, "resume"),
        (req.gates.is_some(), "gates"),
    ];
    if let Some((_, what)) = carried.iter().find(|(set, _)| *set) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("a Box a person starts runs no agent, so its start takes no {what}"),
        )
        .with_hint(ASK_INSTEAD));
    }
    run(
        cx,
        &StartReq {
            no_agent: true,
            ..req
        },
    )
}
