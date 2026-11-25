//! `moor start --place`.

use moor_core::Request;

use super::{n, req, run};

#[test]
fn start_place_takes_a_name_and_no_repo_or_prompt() {
    let made = req(&["start", "p1", "--place"]);
    assert!(
        matches!(&made, Request::Start(s)
            if s.place && s.name == n("p1") && s.repo.is_none() && s.prompt.is_none()),
        "{made:?}"
    );
    for args in [
        &["start", "p1", "--place", "--repo", "o/n"][..],
        &["start", "p1", "--place", "--prompt", "hi"],
        &["start", "--place"],
    ] {
        assert!(run(args).is_err(), "{args:?}");
    }
}

#[test]
fn no_setup_starts_a_box_without_setup_but_not_a_place() {
    assert!(matches!(
        super::req(&["start", "t", "--no-setup"]),
        moor_core::Request::Start(s) if s.no_setup
    ));
    assert!(super::run(&["start", "t", "--place", "--no-setup"]).is_err());
}
