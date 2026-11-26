//! A rule's `allow` list as the text writes it: `METHODS /path` lines, and one
//! `POST /graphql` entry that may carry the GraphQL roots and mutations it lets through.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{Inject, ToolRule};
use crate::graphql_scope::Scope;

/// The allow entry a GraphQL scope hangs from.
pub const GRAPHQL_ENTRY: &str = "POST /graphql";

/// The host whose `POST /graphql` the scope filter checks.
pub const GRAPHQL_HOST: &str = "api.github.com";

/// A rule as the text writes it.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RawRule {
    /// As [`ToolRule::hosts`].
    hosts: Vec<String>,
    /// As [`ToolRule::credential`].
    credential: String,
    /// As [`ToolRule::inject`].
    inject: Inject,
    /// The entries, a GraphQL scope among them.
    allow: AllowList,
    /// As [`ToolRule::box_env`].
    box_env: BTreeMap<String, String>,
    /// As [`ToolRule::requires_repo`].
    requires_repo: bool,
    /// As [`ToolRule::app`].
    app: Vec<String>,
}

impl TryFrom<RawRule> for ToolRule {
    type Error = String;

    fn try_from(raw: RawRule) -> Result<Self, String> {
        let mut graphql = None;
        let mut allow = Vec::with_capacity(raw.allow.0.len());
        for entry in raw.allow.0 {
            match entry {
                Entry::Line(line) => allow.push(line),
                Entry::Scoped(line, scope) => {
                    if line != GRAPHQL_ENTRY {
                        return Err(format!(
                            "only {GRAPHQL_ENTRY:?} takes query and mutation lists, not {line:?}"
                        ));
                    }
                    if graphql.replace(scope).is_some() {
                        return Err(format!("{GRAPHQL_ENTRY:?} is listed twice"));
                    }
                    allow.push(line);
                }
            }
        }
        Ok(Self {
            hosts: raw.hosts,
            credential: raw.credential,
            inject: raw.inject,
            allow,
            box_env: raw.box_env,
            requires_repo: raw.requires_repo,
            graphql,
            app: raw.app,
        })
    }
}

/// One `allow` entry.
enum Entry {
    /// `METHODS /path` or `*`.
    Line(String),
    /// `METHODS /path` with its GraphQL scope under it.
    Scoped(String, Scope),
}

/// The `allow` list.
#[derive(Default)]
struct AllowList(Vec<Entry>);

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entry;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("\"METHODS /path\", or \"POST /graphql\" with query and mutation lists under it")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Entry, E> {
                Ok(Entry::Line(v.to_owned()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entry, A::Error> {
                let Some((line, scope)) = map.next_entry::<String, Scope>()? else {
                    return Err(de::Error::custom(
                        "an allow entry cannot be an empty mapping",
                    ));
                };
                if map.next_key::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("an allow mapping holds one entry"));
                }
                Ok(Entry::Scoped(line, scope))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for AllowList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = AllowList;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of allow entries")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<AllowList, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = seq.next_element()? {
                    out.push(entry);
                }
                Ok(AllowList(out))
            }
        }
        d.deserialize_seq(V)
    }
}
