//! The response check for linked issues. A linked issue (parent, sub-issue, blocked-by,
//! blocking) can be in any repo, and the query cannot say which, so its `title` and `state`
//! are blanked in the response unless its `repository.nameWithOwner` is a Box repo.

use serde_json::Value as Json;

use super::RepoRef;

/// Fields of a linked issue that only a Box repo's issue shows.
const HEADLINE: &[&str] = &["title", "state"];

/// The response paths, by response key, of the linked issues a query reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Linked(Vec<Vec<String>>);

impl Linked {
    /// Whether the query reads no linked issue, so its response needs no check.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Records a path once.
    pub(super) fn add(&mut self, path: Vec<String>) {
        if !self.0.contains(&path) {
            self.0.push(path);
        }
    }

    /// Sets `title` and `state` to null on every linked issue in the response `body` whose
    /// `repository.nameWithOwner` is not one of `repos`, compared without case. An issue
    /// whose repository is missing is blanked too.
    pub fn blank(&self, body: &mut Json, repos: &[RepoRef]) {
        let Some(data) = body.get_mut("data") else {
            return;
        };
        for path in &self.0 {
            visit(data, path, repos);
        }
    }
}

/// Follows `path` from `value`, through every item of each list on the way.
fn visit(value: &mut Json, path: &[String], repos: &[RepoRef]) {
    match (value, path.split_first()) {
        (Json::Array(items), _) => {
            for item in items {
                visit(item, path, repos);
            }
        }
        (Json::Object(obj), Some((key, rest))) => {
            if let Some(next) = obj.get_mut(key) {
                visit(next, rest, repos);
            }
        }
        (Json::Object(obj), None) => {
            let repo = obj
                .get("repository")
                .and_then(|r| r.get("nameWithOwner"))
                .and_then(Json::as_str);
            if !repo.is_some_and(|full| listed(full, repos)) {
                for key in HEADLINE {
                    if let Some(field) = obj.get_mut(*key) {
                        *field = Json::Null;
                    }
                }
            }
        }
        _ => {}
    }
}

/// Whether `owner/name` is one of `repos`, compared without case.
fn listed(full: &str, repos: &[RepoRef]) -> bool {
    full.split_once('/').is_some_and(|(owner, name)| {
        repos
            .iter()
            .any(|r| owner.eq_ignore_ascii_case(&r.owner) && name.eq_ignore_ascii_case(&r.name))
    })
}
