//! The key of the layer above the last setup line.

use moor_core::config::BoxSettings;
use moor_core::domain::layers::{finished_key, FinishedSteps, LayerKey};
use serde_json::Value;

use crate::cx::Cx;
use crate::ops::common::{box_exec, dotfiles};
use crate::store::{BoxRow, Store};

/// The commits of the repos Box `row`'s profile installs from, as text for a key; none when one
/// did not resolve, as what it would install then is unknown.
fn resolved_sources(store: &Store, row: &BoxRow) -> Option<String> {
    let told = store
        .newest_events_of(row.id, "profile_resolved", 1)
        .ok()?
        .pop();
    let Some(told) = told else {
        return Some(String::new());
    };
    let unresolved = told.payload.get("unresolved").and_then(Value::as_array);
    if unresolved.is_some_and(|u| !u.is_empty()) {
        return None;
    }
    let sources = told.payload.get("sources").and_then(Value::as_array);
    let pairs = sources.into_iter().flatten().map(|s| {
        let field = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or_default();
        format!("{}@{}", field("url"), field("sha"))
    });
    let mut pairs: Vec<String> = pairs.collect();
    pairs.sort_unstable();
    Some(pairs.join("\0"))
}

/// The key of the layer above `top` for Box `row`; none when it has no agent install and no
/// profile, or when what those steps would do cannot be told.
pub(super) fn key_of(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    top: &LayerKey,
) -> Option<LayerKey> {
    let install = cfg.install_agent.then(|| cx.engine().agent_install());
    let install = install.flatten().map(|i| i.script);
    let profile = store.box_profile(row.id).ok().flatten();
    if install.is_none() && profile.is_none() {
        return None;
    }
    let manifest = profile.as_ref().and_then(|p| p.manifest.as_deref());
    let manifest = manifest.and_then(|text| moor_core::profile::parse(text).ok());
    let plugins = manifest.and_then(|m| cx.engine().profile_install(&m));
    let sources = resolved_sources(store, row)?;
    let dotfiles = dotfiles::identity(cx, cfg.dotfiles.as_ref())?;
    let env = box_exec::setup_env(cx, row, cfg);
    let env: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let steps = FinishedSteps {
        install: install.as_deref().unwrap_or_default(),
        profile_sha: profile.as_ref().map_or("", |p| p.sha.as_str()),
        plugins: plugins.as_deref().unwrap_or_default(),
        sources: &sources,
        env: &env.join("\0"),
        dotfiles: &dotfiles,
    };
    Some(finished_key(top, &steps))
}
