//! The response check for linked issues. A linked issue (parent, sub-issue, blocked-by,
//! blocking) can be in any repo, and the query cannot say which, so its `title` and `state`
//! are blanked in the response unless its `repository.nameWithOwner` is a Box repo.
//!
//! The response is walked as raw JSON: only the objects and lists on the way to a linked
//! issue are split into their members, and only each linked issue is parsed whole. Every
//! other value is copied through as sent, so the memory taken follows the body's size, not
//! the shape a Box gives it.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
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

    /// Returns the response `body` with `title` and `state` set to null on every linked
    /// issue whose `repository.nameWithOwner` is not one of `repos`, compared without
    /// case. An issue whose repository is missing is blanked too.
    ///
    /// # Errors
    ///
    /// When `body` is not JSON.
    pub fn blank(&self, body: &[u8], repos: &[RepoRef]) -> serde_json::Result<Vec<u8>> {
        let raw: &RawValue = serde_json::from_slice(body)?;
        let rooted: Vec<Vec<String>> = self
            .0
            .iter()
            .map(|path| {
                std::iter::once("data".to_owned())
                    .chain(path.iter().cloned())
                    .collect()
            })
            .collect();
        let paths: Vec<&[String]> = rooted.iter().map(Vec::as_slice).collect();
        let mut out = Vec::with_capacity(body.len());
        walk(raw, &paths, repos, &mut out)?;
        Ok(out)
    }
}

/// Writes `raw` to `out`, blanking the linked issues at the end of `paths`, which start
/// at `raw`. Lists on the way are followed through every item.
fn walk(
    raw: &RawValue,
    paths: &[&[String]],
    repos: &[RepoRef],
    out: &mut Vec<u8>,
) -> serde_json::Result<()> {
    let text = raw.get();
    let level = Level { paths, repos, out };
    match text.trim_start().as_bytes().first() {
        _ if paths.is_empty() => level.out.extend_from_slice(text.as_bytes()),
        Some(b'[') => serde_json::Deserializer::from_str(text).deserialize_seq(level)?,
        Some(b'{') if paths.iter().all(|p| !p.is_empty()) => {
            serde_json::Deserializer::from_str(text).deserialize_map(level)?;
        }
        Some(b'{') => {
            let mut issue: Json = serde_json::from_str(text)?;
            for path in paths {
                visit(&mut issue, path, repos);
            }
            serde_json::to_writer(&mut *level.out, &issue)?;
        }
        _ => level.out.extend_from_slice(text.as_bytes()),
    }
    Ok(())
}

/// Writes one object or list on the way to a linked issue, member by member.
struct Level<'a, 'p> {
    /// The paths still to follow, from this value.
    paths: &'a [&'p [String]],
    /// The Box repos.
    repos: &'a [RepoRef],
    /// Where the response is written.
    out: &'a mut Vec<u8>,
}

impl<'de> Visitor<'de> for Level<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object or list")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<(), A::Error> {
        self.out.push(b'[');
        let mut first = true;
        while let Some(item) = items.next_element::<&'de RawValue>()? {
            if !std::mem::take(&mut first) {
                self.out.push(b',');
            }
            walk(item, self.paths, self.repos, self.out).map_err(de::Error::custom)?;
        }
        self.out.push(b']');
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<(), A::Error> {
        self.out.push(b'{');
        let mut first = true;
        while let Some(Key(key)) = members.next_key()? {
            let value = members.next_value::<&'de RawValue>()?;
            if !std::mem::take(&mut first) {
                self.out.push(b',');
            }
            serde_json::to_writer(&mut *self.out, key.as_ref()).map_err(de::Error::custom)?;
            self.out.push(b':');
            let under: Vec<&[String]> = self
                .paths
                .iter()
                .filter_map(|path| path.split_first())
                .filter(|(step, _)| step.as_str() == key.as_ref())
                .map(|(_, rest)| rest)
                .collect();
            walk(value, &under, self.repos, self.out).map_err(de::Error::custom)?;
        }
        self.out.push(b'}');
        Ok(())
    }
}

/// An object member's name, borrowed from the response unless it holds escapes.
struct Key<'de>(Cow<'de, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(KeyVisitor)
    }
}

/// Reads a [`Key`].
struct KeyVisitor;

impl<'de> Visitor<'de> for KeyVisitor {
    type Value = Key<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a member name")
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
        Ok(Key(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Key<'de>, E> {
        Ok(Key(Cow::Owned(v.to_owned())))
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
