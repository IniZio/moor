//! The config a start uses for each of its repos, now that a repo's config lives in the store.
//!
//! A stored config wins over the repo's `.moor.yaml`. A repo with none imports its file when the
//! account that starts the Box may push to it, since the import writes config every later Box of
//! the repo uses; an account that cannot push uses the file for its own Box only.

use moor_core::config::parse_repo;
use moor_core::domain::decision::BoxRepo;
use moor_core::seam::forge::ForgeRepo;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;

/// What a row imported from a `.moor.yaml` records as the one who wrote it.
const IMPORT: &str = "import";

/// Sets the config text of `boxed` and of each repo beside it to the one the Box uses: the stored
/// config, else the file read at the base. `can_push` is whether the requester's account may push
/// to the primary repo; every repo beside it is one it can push to.
///
/// # Errors
/// A store failure.
pub(crate) fn stored_config(
    cx: &Cx,
    boxed: &mut BoxRepo,
    can_push: bool,
) -> Result<(), CommandError> {
    boxed.moor_yaml = settle(cx, &boxed.repo, boxed.moor_yaml.take(), can_push)?;
    for other in &mut boxed.others {
        other.moor_yaml = settle(cx, &other.repo, other.moor_yaml.take(), true)?;
    }
    Ok(())
}

/// The config text a Box of `repo` uses, given the `file` read at its base.
fn settle(
    cx: &Cx,
    repo: &ForgeRepo,
    file: Option<String>,
    can_push: bool,
) -> Result<Option<String>, CommandError> {
    let store = cx.store()?;
    let mut stored = store.repo_config(repo)?;
    if stored.is_none() {
        let importable = file
            .as_deref()
            .filter(|text| can_push && parse_repo(&label(repo), text).is_ok());
        if let Some(text) = importable {
            if store.insert_repo_config(repo, text, IMPORT)?.is_some() {
                cx.warn(Warning {
                    code: "repo_config_imported".to_owned(),
                    message: format!(
                        "{repo}: .moor.yaml is now imported into moor's repo config, which \
                         replaces it; edit the config in the portal (or with `moor repo config \
                         set`), as changes to the file no longer apply"
                    ),
                });
                return Ok(file);
            }
            // Another start imported it first; it is the row that counts.
            stored = store.repo_config(repo)?;
        }
    }
    let Some(stored) = stored else {
        return Ok(file);
    };
    if let Some(text) = &file {
        if let Some(differs) = differs(repo, &stored.text, text) {
            cx.warn(Warning {
                code: "repo_config_file_ignored".to_owned(),
                message: format!(
                    "{repo}: .moor.yaml {differs} and is ignored; edit the stored repo config \
                     instead"
                ),
            });
        }
    }
    Ok(Some(stored.text))
}

/// How the `file` text differs from the `stored` one, `None` when it means the same.
fn differs(repo: &ForgeRepo, stored: &str, file: &str) -> Option<String> {
    let label = label(repo);
    match (parse_repo(&label, stored), parse_repo(&label, file)) {
        (Ok(stored), Ok(file)) => {
            let keys = stored.changed_keys(&file);
            (!keys.is_empty())
                .then(|| format!("differs from the stored config in {}", keys.join(", ")))
        }
        (_, Err(_)) => Some("does not parse".to_owned()),
        (Err(_), Ok(_)) => Some("differs from the stored config".to_owned()),
    }
}

/// How an error names the text it parsed.
fn label(repo: &ForgeRepo) -> String {
    format!("{repo}:.moor.yaml")
}
