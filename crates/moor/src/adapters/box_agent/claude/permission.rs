//! Whether a permission prompt Claude Code reported is still on its screen.

use moor_core::activity::{menu_line, HookKind, HookRecord};

/// How Claude Code's Notification opens for a permission prompt.
const PERMISSION: &str = "Claude needs your permission";
/// How many trailing non-empty pane lines hold a dialog's choices and footer.
const SCREEN_LINES: usize = 12;
/// Text a permission dialog shows besides its numbered choices.
const DIALOG: [&str; 3] = ["esc to cancel", "automatically block", "do you want"];

/// Whether `record` is a permission prompt and `tail` shows no dialog. Claude Code writes no hook
/// report when such a prompt expires and the turn ends with it.
pub(super) fn gone(record: &HookRecord, tail: &str) -> bool {
    let permission = record.kind == HookKind::Blocked
        && record.options.is_empty()
        && record
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with(PERMISSION));
    let lines: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
    let screen = lines.iter().rev().take(SCREEN_LINES);
    permission
        && !lines.is_empty()
        && !screen.into_iter().any(|line| {
            let lower = line.to_lowercase();
            menu_line(line).is_some() || DIALOG.iter().any(|d| lower.contains(d))
        })
}
