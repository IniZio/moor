//! What the automation page shows of a workflow's file beyond its prompt: the extra Proxy
//! entries its config asks for, as written, so the page never paraphrases a permission.

use moor_core::config::parse_repo;
use serde_json::{json, Value};

/// The config's `proxy.allow` as `{"allow": {rule: [entry, ...]}}`, empty for a workflow with no
/// config, or `{"error": why}` when the stored config no longer parses.
pub(super) fn may(config: Option<&str>) -> Value {
    let Some(text) = config.filter(|t| !t.trim().is_empty()) else {
        return json!({ "allow": {} });
    };
    match parse_repo("workflow config", text) {
        Ok(cfg) => json!({ "allow": cfg.proxy.allow }),
        Err(why) => json!({ "error": why.to_string() }),
    }
}

#[cfg(test)]
mod tests;
