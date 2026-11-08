//! What a steer sends, given what the agent shows.

use moor_core::activity::AgentState;
use moor_core::seam::engine::Steer;
use moor_core::{ErrorKind, SteerReq};

use crate::error::CommandError;

/// What to send for `req` given what the agent shows.
///
/// A blocked agent takes a menu choice only: `--menu N`, or text naming one of its options or
/// its 1-based number.
pub(super) fn choose(
    req: &SteerReq,
    box_name: &str,
    state: Option<&AgentState>,
) -> Result<Steer, CommandError> {
    if req.interrupt {
        return Ok(Steer::Interrupt);
    }
    let waiting = match state {
        Some(AgentState::NeedsInput { question, options }) => Some((question, options)),
        _ => None,
    };
    match (req.menu, req.text.as_deref(), waiting) {
        (Some(n), _, Some(_)) => Ok(Steer::Menu(choice(n)?)),
        (Some(_), _, None) => Err(CommandError::new(
            ErrorKind::Refused,
            format!("the agent of {box_name} is not waiting at a menu"),
        )),
        (None, Some(t), None) => Ok(Steer::Text(t.to_owned())),
        (None, Some(t), Some((question, options))) => {
            answer(t, options).map(Steer::Menu).ok_or_else(|| {
                CommandError::new(
                    ErrorKind::Refused,
                    blocked_hint(box_name, question.as_deref(), options),
                )
            })
        }
        (None, None, _) => Err(CommandError::new(ErrorKind::Usage, "nothing to send")),
    }
}

/// The menu entry `text` picks: an option's label, ignoring case and surrounding space, or its
/// number. With no options read, a lone digit is taken as the number.
fn answer(text: &str, options: &[String]) -> Option<u8> {
    let t = text.trim();
    let by_label = options
        .iter()
        .position(|o| o.trim().eq_ignore_ascii_case(t));
    let by_number = t
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .filter(|n| match options.len() {
            0 => t.len() == 1,
            len => *n <= len,
        });
    let n = by_label.map(|i| i + 1).or(by_number)?;
    u8::try_from(n).ok()
}

/// The refusal for text that answers none of a blocked agent's options.
fn blocked_hint(box_name: &str, question: Option<&str>, options: &[String]) -> String {
    let waiting = question.unwrap_or("input");
    if options.is_empty() {
        return format!(
            "the agent of {box_name} is blocked (waiting for: {waiting}); send the number of an option on its screen, or `moor steer {box_name} --menu N`"
        );
    }
    let listed = options
        .iter()
        .enumerate()
        .map(|(i, o)| format!("{}. {o}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "the agent of {box_name} is blocked (waiting for: {waiting}); send one of its options, by name or number: {listed}"
    )
}

/// A menu number the pane keys can carry.
fn choice(n: u32) -> Result<u8, CommandError> {
    u8::try_from(n)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| CommandError::new(ErrorKind::Usage, format!("no menu choice {n}")))
}
