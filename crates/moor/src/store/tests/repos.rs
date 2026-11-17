//! The repos a Box holds, as the store gives them back.

use moor_core::domain::decision::{Decision, ListedRepo, NewBox, SpecFields};
use moor_core::name::ForgeRepo;

use super::open;
use crate::testing::cx::box_repo;

#[test]
fn a_box_reads_back_every_listed_repo_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let mut repo = box_repo();
    repo.others = vec![
        ListedRepo {
            repo: ForgeRepo::new("o", "lib"),
        },
        ListedRepo {
            repo: ForgeRepo::new("p", "deploy"),
        },
    ];
    let id = store
        .apply(Decision::insert(NewBox {
            name: crate::testing::box_name("multi"),
            repo: Some(repo.clone()),
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.repo, Some(repo));
}

#[test]
fn a_box_of_one_repo_reads_back_as_that_repo_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let repo = box_repo();
    let id = store
        .apply(Decision::insert(NewBox {
            name: crate::testing::box_name("one"),
            repo: Some(repo.clone()),
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.repo, Some(repo));
}
