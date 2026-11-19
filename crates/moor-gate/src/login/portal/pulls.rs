//! A Box's pull requests for its page's header: each repo's, read from the forge by the
//! controller, with the page the header's button opens.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{BoxReq, Request as ControllerRequest};
use moor_core::response::ExitClass;
use moor_core::seam::forge::PullState;
use moor_core::view::pulls::{PullsValue, RepoPull};
use serde_json::{json, Value};

use super::super::account::{failed, json};
use super::super::repos::answered;
use super::super::Session;
use super::param;
use crate::{tools, Gate};

/// Answers the pull request on each repo of the Box the query names, the first repo first.
pub(super) fn read(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let Some(name) = param(query, "box").and_then(|b| BoxName::parse_allow_bare(&b).ok()) else {
        let why = json!({"error": "name a Box"});
        return json(stream, "400 Bad Request", &why);
    };
    let request = ControllerRequest::Pulls(BoxReq { box_name: name });
    let reply = match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "pull request read", &why),
    };
    match reply.class {
        ExitClass::Ok => {}
        ExitClass::Usage => return answered(stream, &reply),
        _ => return json(stream, "404 Not Found", &json!({"error": "no such Box"})),
    }
    match serde_json::from_value::<PullsValue>(reply.value.unwrap_or_default()) {
        Ok(value) => json(stream, "200 OK", &shown(&value)),
        Err(e) => failed(
            stream,
            "pull request read",
            &format!("unreadable answer: {e}"),
        ),
    }
}

/// The header's view of `value`: per repo, the state, and the pull request's page or, before
/// there is one and once the branch is pushed, the page that opens it against the base, or the default branch when the base is
/// a commit, which a pull request cannot target. Each link is built from parts moor knows.
fn shown(value: &PullsValue) -> Value {
    let branch = value.branch.as_deref().unwrap_or_default();
    let repos: Vec<Value> = value
        .repos
        .iter()
        .map(|r| one(r, value.base.as_deref(), branch))
        .collect();
    json!({"branch": value.branch, "repos": repos})
}

/// One repo's entry of [`shown`].
fn one(r: &RepoPull, base: Option<&str>, branch: &str) -> Value {
    let page = format!("https://github.com/{}", r.repo);
    let Some(pull) = &r.pull else {
        let named = base.filter(|b| !(b.len() == 40 && b.bytes().all(|c| c.is_ascii_hexdigit())));
        let range = named.map_or_else(
            || path(branch),
            |b| format!("{}...{}", path(b), path(branch)),
        );
        let (state, url) = if r.pushed || r.unread.is_some() {
            ("none", Some(format!("{page}/compare/{range}?expand=1")))
        } else {
            ("unpushed", None)
        };
        return json!({"repo": r.repo, "state": state, "number": null, "checks": null,
                      "url": url, "unread": r.unread});
    };
    let state = match pull.state {
        PullState::Draft => "draft",
        PullState::Open => "open",
        PullState::ChangesRequested => "changes_requested",
        PullState::Approved => "approved",
        PullState::Merged => "merged",
        PullState::Closed => "closed",
    };
    json!({"repo": r.repo, "state": state, "number": pull.number, "checks": pull.checks,
           "url": format!("{page}/pull/{}", pull.number), "unread": r.unread})
}

/// `git_ref` as URL path segments: unreserved bytes and `/` kept, every other byte escaped.
fn path(git_ref: &str) -> String {
    git_ref
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test assertions index the JSON")]
mod tests {
    use moor_core::name::ForgeRepo;
    use moor_core::seam::forge::{Checks, Pull};

    use super::*;

    fn value(pull: Option<Pull>, base: Option<&str>) -> Value {
        shown(&PullsValue {
            branch: Some("moor/7-fix #2".to_owned()),
            base: base.map(str::to_owned),
            repos: vec![RepoPull {
                repo: ForgeRepo::new("o", "n"),
                pushed: true,
                pull,
                unread: None,
            }],
        })
    }

    #[test]
    fn no_pull_yet_links_the_page_that_opens_one_against_the_base() {
        let got = value(None, Some("main"));
        let repo = &got["repos"][0];
        assert_eq!(repo["state"], "none");
        let want = "https://github.com/o/n/compare/main...moor/7-fix%20%232?expand=1";
        assert_eq!(repo["url"], want);
        let want = "https://github.com/o/n/compare/moor/7-fix%20%232?expand=1";
        let sha = "66166358f418d2007cbeb44ec401062a1cb72094";
        for base in [None, Some(sha)] {
            assert_eq!(value(None, base)["repos"][0]["url"], want, "{base:?}");
        }
    }

    #[test]
    fn a_branch_not_pushed_yet_offers_no_link() {
        let got = shown(&PullsValue {
            branch: Some("moor/7-x".to_owned()),
            base: None,
            repos: vec![RepoPull {
                repo: ForgeRepo::new("o", "n"),
                pushed: false,
                pull: None,
                unread: None,
            }],
        });
        assert_eq!(got["repos"][0]["state"], "unpushed");
        assert_eq!(got["repos"][0]["url"], Value::Null);
    }

    #[test]
    fn a_pull_links_its_own_page_with_its_state_and_checks() {
        let pull = Pull {
            number: 12,
            state: PullState::Approved,
            checks: Some(Checks::Failing),
        };
        let repo = value(Some(pull), Some("main"))["repos"][0].clone();
        assert_eq!(repo["url"], "https://github.com/o/n/pull/12");
        assert_eq!(
            (&repo["state"], &repo["checks"], &repo["number"]),
            (&json!("approved"), &json!("failing"), &json!(12))
        );
    }
}
