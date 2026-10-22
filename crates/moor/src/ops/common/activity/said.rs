//! The agent's last message, read from the hook report that says it went idle.

use moor_core::activity::{Activity, AgentState, HookCache, HookKind, HookRead, Source};

/// Most characters of the agent's last message kept; the end is kept, as it carries the result.
const SAID_MAX: usize = 4000;

/// The agent's last message, cut to its end, when a hook, fresh or cached, reports it idle with one.
pub(super) fn said(
    activity: &Activity,
    hook: &HookRead,
    cache: Option<&HookCache>,
) -> Option<String> {
    if !matches!(activity.source, Source::Hooks | Source::HooksCached)
        || activity.state != AgentState::Idle
    {
        return None;
    }
    let rec = match hook {
        HookRead::Fresh(rec) => Some(rec),
        _ => cache.map(|c| &c.record),
    };
    rec.filter(|rec| rec.kind == HookKind::Idle)
        .and_then(|rec| rec.message.as_deref())
        .map(|text| tail(text, SAID_MAX).to_owned())
}

/// The last `max` characters of `text`.
fn tail(text: &str, max: usize) -> &str {
    let skip = text.chars().count().saturating_sub(max);
    text.char_indices()
        .nth(skip)
        .and_then(|(at, _)| text.get(at..))
        .unwrap_or_default()
}
