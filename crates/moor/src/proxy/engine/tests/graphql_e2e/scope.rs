//! The GraphQL roots and mutations the filter lets through come from the Box's rule.

use moor_core::seam::forge::ForgeRepo;

use super::{github_rule, query, rig_spec, SECRET};

#[test]
fn a_root_the_rule_leaves_out_goes_without_the_credential() {
    let mut rule = github_rule();
    rule.graphql = Some(serde_json::from_str(r#"{"query": ["viewer"]}"#).unwrap());
    let repo = ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    let rig = rig_spec(repo, Vec::new(), vec![rule], None);
    let _ = query(&rig.client(), "o", "r");
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(!only.head.contains(SECRET), "{only:?}");
    rig.stop();
}
