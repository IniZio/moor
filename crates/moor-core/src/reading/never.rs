//! Forbidding words: whether the pinned rules, with the reading's own lines, refuse every
//! request a `never` row lists, or which one they let through and under which rule.

use super::glob::meets;
use super::{Allowed, Line, Pinned, Request};
use crate::config::proxy::{
    default_graphql, ToolRule, APP_CREDENTIAL, GRAPHQL_ENTRY, GRAPHQL_HOST,
};

#[cfg(test)]
mod tests;

/// The first of `requests` the pinned rules or `own` lines let through, with the rule; none
/// when every one is refused. `repo` is the repo the words name, none for each repo of the Box.
pub(super) fn let_through(
    requests: &[Request],
    repo: Option<&str>,
    pinned: &Pinned<'_>,
    own: &[Line],
) -> Option<Allowed> {
    requests.iter().find_map(|request| {
        let host = request.host();
        let (id, rule) = pinned
            .rules
            .iter()
            .find(|(_, r)| r.hosts.iter().any(|h| h.eq_ignore_ascii_case(host)))?;
        let rule_name = match request {
            Request::Http { method, path, .. } => {
                passes(id, rule, own, &format!("{method} {path}"), repo)
            }
            Request::Mutation { name, .. } => {
                let scope = rule.graphql.clone().unwrap_or_else(default_graphql);
                scope.mutation(name)?;
                passes(id, rule, own, GRAPHQL_ENTRY, repo)
            }
        }?;
        Some(Allowed {
            request: request.clone(),
            rule: rule_name,
        })
    })
}

/// The rule that lets `request`, `METHOD /path`, through: `id` by its allow entries or an own
/// line under it, or the App by its lines; none when nothing does.
fn passes(
    id: &str,
    rule: &ToolRule,
    own: &[Line],
    request: &str,
    repo: Option<&str>,
) -> Option<String> {
    let open = rule.allow.is_empty().then(|| "*".to_owned());
    let by_rule = rule.allow.iter().chain(open.as_ref()).chain(under(own, id));
    if by_rule.into_iter().any(|e| meets(e, request, repo)) {
        return Some(id.to_owned());
    }
    let app = rule
        .hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(GRAPHQL_HOST));
    let by_app = rule.app.iter().chain(under(own, APP_CREDENTIAL));
    (app && by_app.into_iter().any(|e| meets(e, request, repo))).then(|| APP_CREDENTIAL.to_owned())
}

/// The entries of `own` under `rule`.
fn under<'a>(own: &'a [Line], rule: &'a str) -> impl Iterator<Item = &'a String> {
    own.iter().filter(move |l| l.rule == rule).map(|l| &l.entry)
}
