//! Text for `repo config get` and `repo config set`.

use moor_core::request::RepoFile;
use moor_core::view::box_config::BoxConfigValue;
use moor_core::view::repo_config::RepoConfigValue;
use serde_json::Value;

/// The stored config of a `repo-config-get` reply as the YAML it holds, so the output can go
/// straight back to `repo config set`; a YAML comment says when there is none.
pub(super) fn get_text(value: &Value) -> Option<String> {
    let view = serde_json::from_value::<RepoConfigValue>(value.clone()).ok()?;
    Some(
        view.text
            .unwrap_or_else(|| format!("# no config is stored for {}\n", view.repo)),
    )
}

/// The line a `box-config-set` reply prints: the Box and whether it now has an override.
pub(super) fn box_line(value: &Value) -> Option<String> {
    let view = serde_json::from_value::<BoxConfigValue>(value.clone()).ok()?;
    let now = if view.set {
        "has its own config"
    } else {
        "reads its repo's config alone"
    };
    Some(format!("{} {now}\n", view.box_name))
}

/// The line a `repo-config-set` reply prints: the repo and the revision it is now at.
pub(super) fn set_line(value: &Value) -> Option<String> {
    let view = serde_json::from_value::<RepoConfigValue>(value.clone()).ok()?;
    let what = match view.file {
        RepoFile::Config => "config",
        RepoFile::Rules => "rules",
    };
    Some(format!(
        "{} {what} is now revision {}\n",
        view.repo, view.revision
    ))
}
