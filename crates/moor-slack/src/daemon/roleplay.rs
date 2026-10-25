//! Turns on role-play when the config lists users.

use std::sync::Arc;

use moor_core::secret::Secret;

use crate::api::{Api, BotIdentity};
use crate::config::Config;
use crate::socket;
use crate::Log;

/// The reaction on every post the front took as role-play.
const ROLEPLAY_MARK: &str = "performing_arts";

/// The role-play setup the config asks for, or why the bot token cannot support it.
pub(super) fn roleplay(
    cfg: &Config,
    me: &BotIdentity,
    api: &Arc<Api>,
    bot_token: &Arc<Secret>,
    log: &Log,
) -> Result<Option<socket::Roleplay>, String> {
    let Some(section) = &cfg.roleplay else {
        return Ok(None);
    };
    let (Some(bot_id), Some(bot_user)) = (me.bot.clone(), me.user.clone()) else {
        return Err("roleplay: slack auth.test did not name the bot's bot_id and user_id".into());
    };
    log(&format!(
        "slack: role-play is on for {}; anyone holding the bot token can act as them",
        section.users.join(", ")
    ));
    let (api, token, warn) = (Arc::clone(api), Arc::clone(bot_token), Arc::clone(log));
    let (react_api, react_token, react_warn) =
        (Arc::clone(&api), Arc::clone(&token), Arc::clone(&warn));
    Ok(Some(socket::Roleplay {
        bot_id,
        bot_user,
        users: section.users.clone(),
        metadata: Box::new(move |channel, root, ts| {
            api.message_metadata(&token, channel, root, ts)
                .inspect_err(|e| warn(&format!("slack: role-play lookup of {channel} {ts}: {e}")))
                .ok()
                .flatten()
        }),
        log: Arc::clone(log),
        mark: Box::new(move |channel, ts| {
            if let Err(e) = react_api.add_reaction(&react_token, channel, ts, ROLEPLAY_MARK) {
                react_warn(&format!("slack: role-play mark on {channel} {ts}: {e}"));
            }
        }),
    }))
}
