//! A firing clones its workflow's `also` repos beside its own, and only those.

use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::name::ForgeRepo;
use moor_core::workflow::{TimeoutMinutes, WorkflowDef, WorkflowName, WorkflowSchedule};

use super::{repo, rig_with, FIRST_DUE};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::forge::repo_at;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

/// Fires a workflow cloning `also` once, under a controller whose own `box.also` lists
/// another repo; returns the repos its Box clones beside its own, as its row and its Proxy
/// see them.
fn fired_with(also: Vec<ForgeRepo>) -> (Vec<ForgeRepo>, Vec<ForgeRepo>) {
    let proxy = FakeProxyHost::new();
    let mut config = UserConfig::default();
    config.box_.also = config_also();
    let mut r = rig_with(config);
    r.cx = r.cx.with_proxy_host(Arc::new(proxy.clone()));
    r.answer_repo();
    for other in [&also[..], &config_also()].concat() {
        r.forge.answer(other, Ok(repo_at("def456", None)));
    }
    let def = WorkflowDef::new(
        WorkflowName::parse("audit").unwrap(),
        WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap(),
        vec![repo()],
        "review the alerts",
        (true, TimeoutMinutes::DEFAULT),
    )
    .and_then(|d| d.with_also(also))
    .unwrap();
    let id =
        r.cx.store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap();
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let box_id = r.latest_box(id);
    let row = r.cx.store().unwrap().box_row(box_id).unwrap().unwrap();
    let listed = row.repo.unwrap().others.into_iter().map(|l| l.repo);
    let proxied = proxy.calls().into_iter().find_map(|c| match c {
        ProxyHostCall::Ensure(spec) if spec.box_id == box_id => Some(spec.other_repos),
        _ => None,
    });
    (
        listed.collect(),
        proxied.expect("the Box's Proxy was ensured"),
    )
}

fn config_also() -> Vec<ForgeRepo> {
    vec![ForgeRepo::new("p", "deploy")]
}

#[test]
fn a_firing_clones_the_workflows_also_repos_and_its_proxy_lets_it_reach_them() {
    let lib = ForgeRepo::new("o", "lib");
    assert_eq!(
        fired_with(vec![lib.clone()]),
        (vec![lib.clone()], vec![lib])
    );
}

#[test]
fn a_workflow_without_also_repos_never_clones_the_controllers_box_also() {
    assert_eq!(fired_with(Vec::new()), (vec![], vec![]));
}
