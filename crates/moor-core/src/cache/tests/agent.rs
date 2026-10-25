//! The agent's install directory as a planned volume.

use super::*;

fn agent() -> AgentCache {
    AgentCache {
        dirs: vec!["~/.local/share/agent".to_owned()],
        check: "true".to_owned(),
    }
}

fn with_agent(repo_yaml: &str, user_yaml: &str) -> CacheLayers {
    CacheLayers {
        agent: Some(agent()),
        ..layers(repo_yaml, user_yaml)
    }
}

fn agent_volume(p: &CachePlan) -> Option<&PlannedVolume> {
    p.volumes().iter().find(|v| v.name.as_str() == "agent")
}

fn plan_for(l: &CacheLayers, user: i64, repo: Option<&ForgeRepo>) -> CachePlan {
    CachePlan::build(UserId::new(user), repo, l, &salt(1)).unwrap()
}

#[test]
fn the_agent_dir_is_a_sealed_volume_in_a_group_of_its_own() {
    let p = plan(&with_agent("", ""));
    let v = agent_volume(&p).expect("the plan keeps the agent dir");
    let paths: Vec<&str> = v.paths.iter().map(CachePath::as_str).collect();
    assert_eq!(paths, ["~/.local/share/agent"]);
    assert_eq!(v.seal(), ["true"]);
    assert!(v.group.ends_with('a'));
    assert_eq!(v.key, format!("{}-agent", v.group));
    let shared = p
        .volumes()
        .iter()
        .any(|o| o.name.as_str() != "agent" && o.group == v.group);
    assert!(!shared, "a failed seal elsewhere never drops it");
}

#[test]
fn the_agent_dir_is_kept_per_user_and_never_per_repo() {
    let l = with_agent("", "");
    let group = |user, repo: Option<&ForgeRepo>| {
        agent_volume(&plan_for(&l, user, repo)).map(|v| v.group.clone())
    };
    let (a, b) = (repo("o", "a"), repo("o", "b"));
    assert_eq!(group(7, Some(&a)), group(7, Some(&b)));
    assert_eq!(group(7, Some(&a)), group(7, None));
    assert_ne!(group(7, Some(&a)), group(8, Some(&a)));
    assert!(
        group(7, None).is_some(),
        "a Box without a repo keeps it too"
    );
}

#[test]
fn no_agent_dir_no_volume() {
    let p = plan(&layers("", ""));
    assert!(agent_volume(&p).is_none());
}

#[test]
fn a_declared_volume_on_the_same_dir_wins() {
    let p = plan(&with_agent(
        "",
        "cache: {volumes: [{name: tools, path: ~/.local/share/agent}]}",
    ));
    assert!(agent_volume(&p).is_none());
    assert!(names(&p).iter().any(|n| n == "tools"));
}
