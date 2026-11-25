//! A repo's stored config: one row per repo, written only at the revision the writer saw.

use moor_core::name::ForgeRepo;
use moor_core::request::RepoFile;

use super::Wrote;
use crate::store::tests::open;

fn repo(s: &str) -> ForgeRepo {
    ForgeRepo::parse(s).unwrap()
}

fn written(w: Wrote) -> super::RepoConfigRow {
    match w {
        Wrote::Written(row) => Some(row),
        Wrote::Stale { .. } => None,
    }
    .expect("the write was not refused")
}

#[test]
fn a_repo_with_no_row_has_no_config() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert_eq!(
        store.repo_text(&repo("o/n"), RepoFile::Config).unwrap(),
        None
    );
}

#[test]
fn the_first_write_is_revision_one_and_each_later_one_adds_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = written(
        store
            .set_repo_text(
                &repo("o/n"),
                RepoFile::Config,
                "setup: [a]\n",
                "operator",
                Some(0),
            )
            .unwrap(),
    );
    assert_eq!((first.revision, first.text.as_str()), (1, "setup: [a]\n"));
    let second = written(
        store
            .set_repo_text(
                &repo("o/n"),
                RepoFile::Config,
                "setup: [b]\n",
                "user 3",
                Some(1),
            )
            .unwrap(),
    );
    assert_eq!((second.revision, second.edited_by.as_str()), (2, "user 3"));
    assert_eq!(
        store.repo_text(&repo("o/n"), RepoFile::Config).unwrap(),
        Some(second)
    );
}

#[test]
fn a_write_at_a_revision_the_row_has_left_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .set_repo_text(
            &repo("o/n"),
            RepoFile::Config,
            "setup: [a]\n",
            "operator",
            Some(0),
        )
        .unwrap();
    let stale = store
        .set_repo_text(
            &repo("o/n"),
            RepoFile::Config,
            "setup: [b]\n",
            "operator",
            Some(7),
        )
        .unwrap();
    assert_eq!(stale, Wrote::Stale { current: Some(1) });
    let create = store
        .set_repo_text(
            &repo("o/n"),
            RepoFile::Config,
            "setup: [b]\n",
            "operator",
            Some(0),
        )
        .unwrap();
    assert_eq!(create, Wrote::Stale { current: Some(1) });
    let row = store
        .repo_text(&repo("o/n"), RepoFile::Config)
        .unwrap()
        .unwrap();
    assert_eq!((row.revision, row.text.as_str()), (1, "setup: [a]\n"));
    let missing = store
        .set_repo_text(
            &repo("o/other"),
            RepoFile::Config,
            "setup: [b]\n",
            "operator",
            Some(1),
        )
        .unwrap();
    assert_eq!(missing, Wrote::Stale { current: None });
}

#[test]
fn a_write_with_no_expected_revision_replaces_whatever_the_row_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let created = written(
        store
            .set_repo_text(
                &repo("o/n"),
                RepoFile::Config,
                "setup: [a]\n",
                "operator",
                None,
            )
            .unwrap(),
    );
    assert_eq!(created.revision, 1);
    let replaced = written(
        store
            .set_repo_text(
                &repo("o/n"),
                RepoFile::Config,
                "setup: [b]\n",
                "operator",
                None,
            )
            .unwrap(),
    );
    assert_eq!(
        (replaced.revision, replaced.text.as_str()),
        (2, "setup: [b]\n")
    );
}

#[test]
fn an_import_never_replaces_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = store
        .insert_repo_text(&repo("o/n"), RepoFile::Config, "setup: [a]\n", "import")
        .unwrap()
        .unwrap();
    assert_eq!((first.revision, first.edited_by.as_str()), (1, "import"));
    assert_eq!(
        store
            .insert_repo_text(&repo("o/n"), RepoFile::Config, "setup: [b]\n", "import")
            .unwrap(),
        None
    );
    assert_eq!(
        store.repo_text(&repo("o/n"), RepoFile::Config).unwrap(),
        Some(first)
    );
}

#[test]
fn the_case_of_owner_and_name_does_not_make_a_second_row() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .set_repo_text(
            &repo("IniZio/Moor"),
            RepoFile::Config,
            "setup: [a]\n",
            "operator",
            Some(0),
        )
        .unwrap();
    let row = store
        .repo_text(&repo("inizio/moor"), RepoFile::Config)
        .unwrap()
        .unwrap();
    assert_eq!(row.text, "setup: [a]\n");
}

#[test]
fn a_repo_keeps_its_rules_apart_from_its_config() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let n = repo("o/n");
    written(
        store
            .set_repo_text(&n, RepoFile::Config, "setup: [a]\n", "operator", Some(0))
            .unwrap(),
    );
    assert_eq!(store.repo_text(&n, RepoFile::Rules).unwrap(), None);
    let rules = written(
        store
            .set_repo_text(&n, RepoFile::Rules, "rules: []\n", "user 3", Some(0))
            .unwrap(),
    );
    assert_eq!(rules.revision, 1);
    assert_eq!(
        store.repo_text(&n, RepoFile::Config).unwrap().unwrap().text,
        "setup: [a]\n"
    );
    assert!(matches!(
        store
            .set_repo_text(&n, RepoFile::Rules, "rules: []\n", "user 3", Some(0))
            .unwrap(),
        Wrote::Stale { current: Some(1) }
    ));
}
