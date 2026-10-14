use super::recorded_workdir;

#[test]
fn a_workdir_under_the_root_round_trips() {
    let path = recorded_workdir("/box-1").unwrap();
    assert_eq!(path.as_str(), "/box-1");
}

#[test]
fn a_nested_workdir_round_trips() {
    let path = recorded_workdir("/work/repos/box-1").unwrap();
    assert_eq!(path.as_str(), "/work/repos/box-1");
}

#[test]
fn a_workdir_whose_root_holds_a_parent_component_round_trips() {
    let path = recorded_workdir("/p/bin/../box/work/moor-rn-1").unwrap();
    assert_eq!(path.as_str(), "/p/bin/../box/work/moor-rn-1");
}

#[test]
fn a_relative_or_empty_workdir_is_rejected() {
    assert!(recorded_workdir("work/box-1").is_none());
    assert!(recorded_workdir("/").is_none());
    assert!(recorded_workdir("").is_none());
}
