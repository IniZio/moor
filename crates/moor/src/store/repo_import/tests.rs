//! The import of the repo configs earlier Boxes carried: the newest Box wins, text that does not
//! parse stays out, an existing row stays, and it runs once.

use std::sync::atomic::{AtomicUsize, Ordering};

use moor_core::name::ForgeRepo;
use moor_schema::tables::{Boxes, Repos};
use sea_query::Query;

use super::Imported;
use crate::store::db::DbExt as _;
use crate::store::tests::open;
use crate::store::Store;

/// Gives each legacy Box its own name.
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn repo(s: &str) -> ForgeRepo {
    ForgeRepo::parse(s).unwrap()
}

/// A Box row as a controller from before the repo config table left it.
fn legacy_box(store: &Store, repo: &str, text: Option<&str>, others: Option<&str>) {
    let name = format!("b{}", NEXT.fetch_add(1, Ordering::Relaxed));
    store
        .writer()
        .run(
            Query::insert()
                .into_table(Boxes::Table)
                .columns([
                    Boxes::Name,
                    Boxes::Repo,
                    Boxes::RepoConfig,
                    Boxes::OtherRepos,
                    Boxes::CreatedAt,
                ])
                .values_panic([
                    name.into(),
                    repo.into(),
                    text.into(),
                    others.into(),
                    0_i64.into(),
                ]),
        )
        .unwrap();
}

#[test]
fn the_newest_box_of_each_repo_gives_its_text() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    legacy_box(&store, "o/n", Some("setup: [old]\n"), None);
    legacy_box(&store, "O/N", Some("setup: [new]\n"), None);
    legacy_box(&store, "o/other", Some("setup: [x]\n"), None);

    let done = store.import_repo_configs().unwrap();

    let got = store.repo_config(&repo("o/n")).unwrap().unwrap();
    assert_eq!(
        (got.text.as_str(), got.revision, got.edited_by.as_str()),
        ("setup: [new]\n", 1, "import")
    );
    assert!(store.repo_config(&repo("o/other")).unwrap().is_some());
    assert_eq!(done.stored.len(), 2);
    assert_eq!(done.skipped, vec![]);
}

#[test]
fn text_that_does_not_parse_is_left_out_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    legacy_box(&store, "o/bad", Some("nonsense_key: 1\n"), None);
    legacy_box(&store, "o/good", Some("setup: [a]\n"), None);

    let done = store.import_repo_configs().unwrap();

    assert_eq!(store.repo_config(&repo("o/bad")).unwrap(), None);
    assert!(store.repo_config(&repo("o/good")).unwrap().is_some());
    assert_eq!(done.skipped.len(), 1);
    assert_eq!(done.skipped.first().map(|s| &s.0), Some(&repo("o/bad")));
}

#[test]
fn a_repo_that_has_a_row_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .set_repo_config(&repo("o/n"), "setup: [mine]\n", "operator", Some(0))
        .unwrap();
    legacy_box(&store, "o/n", Some("setup: [boxes]\n"), None);

    let done = store.import_repo_configs().unwrap();

    let kept = store.repo_config(&repo("o/n")).unwrap().unwrap();
    assert_eq!(
        (kept.text.as_str(), kept.edited_by.as_str()),
        ("setup: [mine]\n", "operator")
    );
    assert_eq!(done, Imported::default());
}

#[test]
fn a_newest_box_with_no_text_means_the_repo_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    legacy_box(&store, "o/n", Some("setup: [old]\n"), None);
    legacy_box(&store, "o/n", None, None);
    legacy_box(&store, "o/blank", Some("  \n"), None);

    store.import_repo_configs().unwrap();

    assert_eq!(store.repo_config(&repo("o/n")).unwrap(), None);
    assert_eq!(store.repo_config(&repo("o/blank")).unwrap(), None);
}

#[test]
fn a_repo_only_listed_beside_another_is_imported_from_the_listing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let others =
        r#"[{"repo":"o/lib","moor_yaml":"setup: [lib]\n"},{"repo":"o/bare","moor_yaml":null}]"#;
    legacy_box(&store, "o/app", Some("setup: [app]\n"), Some(others));

    store.import_repo_configs().unwrap();

    let lib = store.repo_config(&repo("o/lib")).unwrap().unwrap();
    assert_eq!(lib.text, "setup: [lib]\n");
    assert_eq!(store.repo_config(&repo("o/bare")).unwrap(), None);
}

#[test]
fn it_runs_once_so_a_deleted_row_is_not_brought_back() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    legacy_box(&store, "o/n", Some("setup: [a]\n"), None);
    assert_eq!(store.import_repo_configs().unwrap().stored.len(), 1);

    store
        .writer()
        .run(Query::delete().from_table(Repos::Table))
        .unwrap();
    legacy_box(&store, "o/later", Some("setup: [b]\n"), None);

    assert_eq!(store.import_repo_configs().unwrap(), Imported::default());
    assert_eq!(store.repo_config(&repo("o/n")).unwrap(), None);
    assert_eq!(store.repo_config(&repo("o/later")).unwrap(), None);
}
