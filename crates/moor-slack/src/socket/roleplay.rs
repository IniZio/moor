//! The bot's own posts that act as an allow-listed user, for test automation.

use serde_json::Value;

use super::Mention;

/// Looks up a message's metadata by channel, thread root and ts.
pub(crate) type MetadataLookup =
    Box<dyn Fn(&str, &str, &str) -> Option<serde_json::Value> + Send + Sync>;

/// The bot's own posts that may act as another user: those carrying `moor_roleplay` metadata
/// for one of `users`. Built only when the config turns role-play on.
pub(crate) struct Roleplay {
    /// The bot's `bot_id`, as `auth.test` reports it.
    pub(crate) bot_id: String,
    /// The bot's user id, which a post names to mention the bot.
    pub(crate) bot_user: String,
    /// Users the bot may act as.
    pub(crate) users: Vec<String>,
    /// The metadata of the message `ts` in `channel` under thread root `root`, for an event
    /// that arrived without it.
    pub(crate) metadata: MetadataLookup,
    /// Where each role-played message is logged.
    pub(crate) log: crate::Log,
}

/// A `message` the bot itself posted, as the mention or reply of the user its `moor_roleplay`
/// metadata names, if that user is one `roleplay` allows. Slack sends no `app_mention` for the
/// bot's own posts, so a post naming the bot is a mention here.
pub(super) fn roleplayed(payload: &Value, event: &Value, roleplay: &Roleplay) -> Option<Mention> {
    let field = |key: &str| event.get(key)?.as_str().map(str::to_owned);
    let own = field("bot_id")? == roleplay.bot_id;
    let plain = matches!(field("subtype").as_deref(), None | Some("bot_message"));
    if !own || !plain || field("type")? != "message" {
        return None;
    }
    let event_id = payload.get("event_id")?.as_str()?.to_owned();
    let (channel, ts, text) = (field("channel")?, field("ts")?, field("text")?);
    let thread_ts = field("thread_ts").filter(|t| *t != ts);
    let root = thread_ts.as_deref().unwrap_or(&ts);
    let metadata = match event.get("metadata") {
        Some(metadata) => metadata.clone(),
        None => (roleplay.metadata)(&channel, root, &ts)?,
    };
    let user = allowed_user(&metadata, roleplay)?;
    let mentions_bot = text.contains(&format!("<@{}", roleplay.bot_user));
    if thread_ts.is_none() && !mentions_bot {
        return None;
    }
    (roleplay.log)(&format!(
        "slack: role-play message in {channel} at {ts} as {user}"
    ));
    Some(Mention {
        event_id,
        channel,
        user,
        text,
        ts,
        thread_ts,
        reply: !mentions_bot,
    })
}

/// The user `moor_roleplay` metadata names, if `roleplay` allows acting as them.
fn allowed_user(metadata: &Value, roleplay: &Roleplay) -> Option<String> {
    if metadata.get("event_type")?.as_str()? != "moor_roleplay" {
        return None;
    }
    let user = metadata.get("event_payload")?.get("user")?.as_str()?;
    roleplay
        .users
        .iter()
        .any(|u| u == user)
        .then(|| user.to_owned())
}
