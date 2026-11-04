//! Tests for opening the store.

use super::Database;
use crate::env::Env;

/// The database an env built from literal pairs names.
fn database(pairs: &[(&str, &str)]) -> Result<Option<Database>, crate::store::StoreError> {
    Database::from_env(&Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v))))
}

#[test]
fn the_reader_cap_defaults_to_four_and_is_read_from_the_env() {
    let url = ("MOOR_DATABASE_URL", "postgres://u@h/db");
    assert_eq!(database(&[url]).unwrap().unwrap().readers, 4);
    let set = database(&[url, ("MOOR_DATABASE_READERS", "7")]);
    assert_eq!(set.unwrap().unwrap().readers, 7);
    let empty = database(&[url, ("MOOR_DATABASE_READERS", "")]);
    assert_eq!(empty.unwrap().unwrap().readers, 4);
    assert!(database(&[("MOOR_DATABASE_READERS", "7")])
        .unwrap()
        .is_none());
}

#[test]
fn a_reader_cap_that_is_not_a_positive_count_is_refused() {
    for bad in ["0", "-1", "two", "1.5"] {
        let err = database(&[
            ("MOOR_DATABASE_URL", "postgres://u@h/db"),
            ("MOOR_DATABASE_READERS", bad),
        ])
        .unwrap_err()
        .to_string();
        assert!(err.starts_with("MOOR_DATABASE_READERS:"), "{bad}: {err}");
    }
}
