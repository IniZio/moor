//! Default rules, user overrides, and `{repo}` filling.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::template::{check_slots, fill_any_repo, fill_repo, uses_repo, ALLOW_SLOTS, ANY_REPO};
use super::{parse_rules, ProxyConfig, ToolRule, DEFAULT_HEADER};
use crate::config::ConfigError;
use crate::name::ForgeRepo;

/// The embedded default rules.
const DEFAULTS: &str = include_str!("defaults.yaml");

/// The embedded rules a front's Proxy may serve; never part of a Box's rules.
const FRONT_DEFAULTS: &str = include_str!("fronts.yaml");

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
    /// The name of the Box user's secret the rule injects.
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

/// The rules a front's Proxy may serve, resolved and sorted by name.
#[must_use]
pub fn front_rules() -> Vec<ResolvedRule> {
    let rules = parse_rules("fronts.yaml", FRONT_DEFAULTS).unwrap_or_default();
    rules
        .into_iter()
        .filter_map(|(name, rule)| resolve_one(name, &rule, &[]).ok())
        .collect()
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
    check_slots(path, ALLOW_SLOTS)?;
    if path.contains(ANY_REPO) && path.contains("receive-pack") {
        return Err(format!(
            "{ANY_REPO} may only fetch; push needs {{repo}}, got {entry:?}"
        ));
    }
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

/// Defaults overlaid with the user's rules, filled for the Box's `repos`.
///
/// A user rule named like a default extends it: see `ToolRule::over`. An allow entry with
/// `{repo}` repeats once per repo in `repos`, and `{any_repo}` matches any one repo;
/// `box_env` names only the first repo. Rules that use `{repo}` are left out when `repos` is
/// empty, unless they list an `{any_repo}` entry and `fetch_without_repo` is set, in which
/// case only their `{repo}` entries are. Rules come out sorted by name.
///
/// # Errors
/// Returns [`ConfigError`] when the repo owner or name has characters outside
/// `[A-Za-z0-9._-]` (or is empty, `.` or `..`), or a rule is invalid.
pub fn resolve_rules(
    user: &ProxyConfig,
    repos: &[ForgeRepo],
    fetch_without_repo: bool,
) -> Result<Vec<ResolvedRule>, ConfigError> {
    for r in repos {
        check_repo_part("owner", &r.owner)?;
        check_repo_part("name", &r.name)?;
    }
    let merged = user.merged_rules();
    let mut out = Vec::new();
    for (name, rule) in merged {
        let serves_any = fetch_without_repo && rule.allow.iter().any(|a| a.contains(ANY_REPO));
        if repos.is_empty() && (rule.requires_repo || (rule.needs_repo() && !serves_any)) {
            continue;
        }
        out.push(resolve_one(name, &rule, repos)?);
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

/// `path` once per Box repo if it names `{repo}`, with `{any_repo}` as a glob.
fn expand_path(path: &str, repos: &[ForgeRepo]) -> Vec<String> {
    let path = fill_any_repo(path);
    if uses_repo(&path) {
        repos
            .iter()
            .map(|r| fill_repo(&path, &r.owner, &r.name))
            .collect()
    } else {
        vec![path]
    }
}

/// Fills one rule.
fn resolve_one(
    name: String,
    rule: &ToolRule,
    repos: &[ForgeRepo],
) -> Result<ResolvedRule, ConfigError> {
    let at = format!("proxy.rules.{name}");
    rule.validate(&at)?;
    let primary = repos.first();
    let fill = |s: &str| primary.map_or_else(|| s.to_owned(), |r| fill_repo(s, &r.owner, &r.name));
    let mut endpoint_rules = Vec::new();
    for (i, entry) in rule.allow.iter().enumerate() {
        match parse_allow(entry).map_err(|w| ConfigError::new(format!("{at}.allow[{i}]"), w))? {
            Allow::Any => {}
            Allow::Rule(_, path) if repos.is_empty() && uses_repo(&path) => {}
            Allow::Rule(methods, path) => {
                for path in expand_path(&path, repos) {
                    endpoint_rules.extend(methods.iter().map(|method| EndpointRule {
                        method: method.clone(),
                        path: path.clone(),
                    }));
                }
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
            .filter(|(_, v)| primary.is_some() || !uses_repo(v))
            .map(|(k, v)| (k.clone(), fill(v)))
            .collect(),
        name,
    })
}
