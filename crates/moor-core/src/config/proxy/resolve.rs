//! Default rules, user overrides, and `{repo}` filling.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::template::{check_slots, fill_repo, REPO_SLOTS};
use super::{parse_rules, ProxyConfig, ToolRule, DEFAULT_HEADER};
use crate::config::{ConfigError, CredentialSource, Identity};

/// The embedded default rules.
const DEFAULTS: &str = include_str!("defaults.yaml");

/// A method and path glob the proxy lets through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointRule {
    /// Upper-case method, or `*` for any.
    pub method: String,
    /// Path glob: `*` is one segment, `**` any number.
    pub path: String,
}

/// How a resolved rule adds its credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedInject {
    /// Set `header` to `form` with `{secret}` replaced.
    Header {
        /// Header name.
        header: String,
        /// Value template containing `{secret}`.
        form: String,
    },
    /// HTTP Basic with this user and the credential as password.
    BasicAuth {
        /// Basic-auth user name.
        user: String,
    },
}

/// A rule with the Box repo filled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRule {
    /// Rule name.
    pub name: String,
    /// Lower-cased exact domains.
    pub hosts: Vec<String>,
    /// `credentials:` entry name, or `forge`.
    pub credential: String,
    /// Credential injection.
    pub inject: ResolvedInject,
    /// Allowed requests; empty means no path list.
    pub endpoint_rules: Vec<EndpointRule>,
    /// Box environment variables.
    pub box_env: BTreeMap<String, String>,
}

/// The embedded default rules by name.
#[must_use]
pub fn default_rules() -> BTreeMap<String, ToolRule> {
    parse_rules("defaults.yaml", DEFAULTS).unwrap_or_default()
}

/// One parsed `allow` entry.
enum Allow {
    /// `*`: no path list.
    Any,
    /// Methods (`*` for any) and a path.
    Rule(Vec<String>, String),
}

/// Parses `*` or `METHODS /path`, checking slots.
fn parse_allow(entry: &str) -> Result<Allow, String> {
    if entry == "*" {
        return Ok(Allow::Any);
    }
    let bad = || format!("expected \"*\" or \"METHODS /path\", got {entry:?}");
    let (methods, path) = entry.split_once(' ').ok_or_else(bad)?;
    let method_ok = |m: &str| !m.is_empty() && m.chars().all(|c| c.is_ascii_uppercase());
    let any = methods == "*";
    if !any && !methods.split('|').all(method_ok) {
        return Err(bad());
    }
    if !path.starts_with('/') || path.contains(char::is_whitespace) {
        return Err(bad());
    }
    check_slots(path, REPO_SLOTS)?;
    let list = if any {
        vec!["*".to_owned()]
    } else {
        methods.split('|').map(str::to_owned).collect()
    };
    Ok(Allow::Rule(list, path.to_owned()))
}

/// Validates every `allow` entry of the rule at `at`; `*` stands alone.
pub(super) fn validate_allow(allow: &[String], at: &str) -> Result<(), ConfigError> {
    for (i, entry) in allow.iter().enumerate() {
        let here = format!("{at}.allow[{i}]");
        match parse_allow(entry).map_err(|w| ConfigError::new(here.clone(), w))? {
            Allow::Any if allow.len() > 1 => {
                return Err(ConfigError::new(
                    here,
                    "\"*\" cannot be combined with other entries",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Defaults overlaid with the user's rules, filled for `repo` (owner, name).
///
/// A user rule named like a default extends it: see `ToolRule::over`. Rules that use
/// `{repo}` are left out when there is no repo, and `requires_you` rules unless
/// `identity` is the person's own. Rules come out sorted by name.
///
/// # Errors
/// Returns [`ConfigError`] when the repo owner or name has characters outside
/// `[A-Za-z0-9._-]` (or is empty, `.` or `..`), when a rule that is used names a credential missing
/// from `credentials` (default rules are checked here, user rules on load), or
/// is otherwise invalid.
pub fn resolve_rules(
    user: &ProxyConfig,
    credentials: &BTreeMap<String, CredentialSource>,
    repo: Option<(&str, &str)>,
    identity: Identity,
) -> Result<Vec<ResolvedRule>, ConfigError> {
    if let Some((owner, name)) = repo {
        check_repo_part("owner", owner)?;
        check_repo_part("name", name)?;
    }
    let merged = user.merged_rules();
    let defaults = default_rules();
    let mut out = Vec::new();
    for (name, rule) in merged {
        if (rule.needs_repo() && repo.is_none()) || (rule.requires_you && !identity.is_you()) {
            continue;
        }
        if defaults.contains_key(&name) {
            rule.check_default_credential(&name, credentials)?;
        } else {
            rule.check_credential(&format!("proxy.rules.{name}"), credentials)?;
        }
        out.push(resolve_one(name, &rule, repo)?);
    }
    Ok(out)
}

/// A repo owner or name safe to splice into a path: `[A-Za-z0-9._-]+`, not `.` or `..`.
fn check_repo_part(what: &str, v: &str) -> Result<(), ConfigError> {
    let ok = !v.is_empty()
        && v != "."
        && v != ".."
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(ConfigError::new(
            "",
            format!("repo {what} {v:?} must match [A-Za-z0-9._-]+ and not be . or .."),
        ))
    }
}

/// Fills one rule.
fn resolve_one(
    name: String,
    rule: &ToolRule,
    repo: Option<(&str, &str)>,
) -> Result<ResolvedRule, ConfigError> {
    let at = format!("proxy.rules.{name}");
    rule.validate(&at)?;
    let fill =
        |s: &str| repo.map_or_else(|| s.to_owned(), |(owner, name)| fill_repo(s, owner, name));
    let mut endpoint_rules = Vec::new();
    for (i, entry) in rule.allow.iter().enumerate() {
        match parse_allow(entry).map_err(|w| ConfigError::new(format!("{at}.allow[{i}]"), w))? {
            Allow::Any => {}
            Allow::Rule(methods, path) => {
                let path = fill(&path);
                endpoint_rules.extend(methods.into_iter().map(|method| EndpointRule {
                    method,
                    path: path.clone(),
                }));
            }
        }
    }
    let inject = match (&rule.inject.form, &rule.inject.basic_user) {
        (Some(form), _) => ResolvedInject::Header {
            header: rule
                .inject
                .header
                .clone()
                .unwrap_or_else(|| DEFAULT_HEADER.to_owned()),
            form: form.clone(),
        },
        (None, user) => ResolvedInject::BasicAuth {
            user: user.clone().unwrap_or_default(),
        },
    };
    Ok(ResolvedRule {
        hosts: rule
            .hosts
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .collect(),
        credential: rule.credential.clone(),
        inject,
        endpoint_rules,
        box_env: rule
            .box_env
            .iter()
            .map(|(k, v)| (k.clone(), fill(v)))
            .collect(),
        name,
    })
}
