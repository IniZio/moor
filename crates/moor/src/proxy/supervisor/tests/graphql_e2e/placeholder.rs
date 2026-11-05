//! Where the placeholder on an out-of-scope GraphQL request comes from.

use std::collections::BTreeMap;

use moor_core::config::proxy::ResolvedRule;
use moor_core::seam::forge::ForgeRepo;

use super::{github_rule, query, rig_spec, SECRET};
use crate::proxy::supervisor::listen::github_placeholder;

/// The `Authorization` GitHub sees on an out-of-scope query with `rules`.
fn out_of_scope_authorization(rules: Vec<ResolvedRule>) -> Option<String> {
    let repo = ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    let rig = rig_spec(repo, Vec::new(), rules, None);
    let (status, _) = query(&rig.client(), "victim", "secrets");
    assert_eq!(status, 401);
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(!only.head.contains(SECRET), "{only:?}");
    let authorization = only.authorization.clone();
    rig.live.unwrap().stop();
    authorization
}

/// The GitHub rule with exactly this `box_env`.
fn rule_with_env(env: &[(&str, &str)]) -> ResolvedRule {
    ResolvedRule {
        box_env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        ..github_rule()
    }
}

#[test]
fn a_user_gh_token_is_the_placeholder_the_box_also_holds() {
    let rule = rule_with_env(&[("GH_TOKEN", "gho_user-choice")]);
    assert_eq!(
        out_of_scope_authorization(vec![rule]).as_deref(),
        Some("bearer gho_user-choice")
    );
}

#[test]
fn a_later_rule_for_the_host_replaces_the_placeholder_as_it_does_in_the_box_env() {
    let other_host = ResolvedRule {
        hosts: vec!["example.com".to_owned()],
        ..rule_with_env(&[("GH_TOKEN", "gho_other_host")])
    };
    let rules = [
        rule_with_env(&[("GH_TOKEN", "gho_first")]),
        rule_with_env(&[("GH_TOKEN", "gho_second")]),
        other_host,
    ];
    assert_eq!(github_placeholder(&rules).as_deref(), Some("gho_second"));
}

#[test]
fn a_rule_without_gh_token_sends_no_authorization() {
    assert_eq!(out_of_scope_authorization(vec![rule_with_env(&[])]), None);
}

#[test]
fn a_gh_token_that_is_not_one_header_token_sends_no_authorization() {
    for bad in ["", "two words", "x\r\nx-injected: yes"] {
        let rule = rule_with_env(&[("GH_TOKEN", bad)]);
        assert_eq!(out_of_scope_authorization(vec![rule]), None, "{bad:?}");
    }
}
