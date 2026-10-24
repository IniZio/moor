//! The repo work tree as a planned volume.

use super::*;

fn work_tree(p: &CachePlan) -> Option<&PlannedVolume> {
    p.volumes().iter().find(|v| v.name.as_str() == "worktree")
}

#[test]
fn a_repo_box_keeps_its_work_tree_sealed_in_a_group_of_its_own() {
    let p = plan(&layers("", ""));
    let v = work_tree(&p).expect("a repo Box plans its work tree");
    let paths: Vec<&str> = v.paths.iter().map(CachePath::as_str).collect();
    assert_eq!(paths, ["~/moor/repos/r"]);
    assert_eq!(v.group, format!("{}w", p.scope().unwrap()));
    assert_eq!(v.key, format!("{}-worktree", v.group));
    assert!(!v.seal().is_empty(), "the work tree is sealed");
    assert_eq!(v.size_gb.map(NonZeroU32::get), Some(40));
    assert_eq!(p.work_tree(), Some("repos/r"));
    let docker = p
        .volumes()
        .iter()
        .find(|v| v.name.as_str() == "docker")
        .unwrap();
    assert_ne!(
        docker.group, v.group,
        "a failed docker seal never throws the tree away"
    );
}

#[test]
fn the_work_tree_is_kept_even_with_every_preset_off() {
    let p = plan(&layers(
        "cache: {presets: false}",
        "cache: {presets: false}",
    ));
    assert_eq!(names(&p), ["worktree"]);
    assert!(p.scope().is_some());
}

#[test]
fn no_repo_or_work_tree_false_in_either_layer_means_no_work_tree() {
    let l = layers("", "");
    let norepo = CachePlan::build(UserId::new(7), None, &l, &salt(1)).unwrap();
    assert_eq!(work_tree(&norepo), None);
    assert_eq!(norepo.work_tree(), None);
    for (repo_yaml, user_yaml) in [
        ("cache: {work_tree: false}", ""),
        ("", "cache: {work_tree: false}"),
    ] {
        let p = plan(&layers(repo_yaml, user_yaml));
        assert_eq!(work_tree(&p), None, "{repo_yaml} {user_yaml}");
        assert_eq!(p.work_tree(), None);
        assert!(names(&p).contains(&"npm".to_owned()), "the presets stay");
    }
}

#[test]
fn a_size_comes_from_the_repo_then_the_operator() {
    let size = |repo_yaml: &str, user_yaml: &str| {
        let p = plan(&layers(repo_yaml, user_yaml));
        work_tree(&p).and_then(|v| v.size_gb).map(NonZeroU32::get)
    };
    assert_eq!(size("cache: {work_tree: {size_gb: 80}}", ""), Some(80));
    assert_eq!(size("", "cache: {work_tree: {size_gb: 60}}"), Some(60));
    assert_eq!(
        size(
            "cache: {work_tree: {size_gb: 80}}",
            "cache: {work_tree: {size_gb: 60}}"
        ),
        Some(80)
    );
    assert_eq!(size("cache: {work_tree: true}", ""), Some(40));
    for bad in [
        "cache: {work_tree: {size: 1}}",
        "cache: {work_tree: {size_gb: 0}}",
    ] {
        assert!(
            parse_repo(".moor.yaml", bad).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn a_user_volume_under_the_work_root_is_still_refused() {
    for p in ["~/moor/repos/r", "~/moor/repos", "~/moor/x"] {
        assert!(
            refusal(p).contains("moor writes ~/moor"),
            "{p}: {}",
            refusal(p)
        );
    }
}

#[test]
fn a_repo_name_no_cache_path_takes_means_no_work_tree() {
    let l = layers("", "");
    for name in ["..", ".", "a b", "a/b", ""] {
        let p = CachePlan::build(UserId::new(7), Some(&repo("o", name)), &l, &salt(1)).unwrap();
        assert_eq!(work_tree(&p), None, "{name:?}");
    }
}

#[test]
fn a_declared_volume_named_worktree_displaces_it() {
    let p = plan(&layers(
        "cache: {volumes: [{name: worktree, path: ~/wt}]}",
        "",
    ));
    let v = work_tree(&p).unwrap();
    assert_eq!(v.paths, [CachePath::parse("~/wt").unwrap()]);
    assert_eq!(p.work_tree(), None);
}
