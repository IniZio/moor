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

mod local_mode {
    use std::fs::File;
    use std::io::Write as _;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::store::{Store, StoreError};
    use crate::testing::clock::FakeClock;
    use moor_core::seam::clock::Ts;

    const SHORT: Duration = Duration::from_millis(100);

    fn open(dir: &std::path::Path) -> Result<Store, StoreError> {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        Store::open_in_within(dir, None, clock, SHORT)
    }

    /// A lock held as a controller on another pid holds it.
    fn controller_holds(dir: &std::path::Path) -> File {
        let file = File::create(dir.join("controller.lock")).unwrap();
        file.lock().unwrap();
        writeln!(&file, "4242\n/run/moor.sock").unwrap();
        file
    }

    #[test]
    fn is_refused_while_a_controller_serves_the_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let _controller = controller_holds(tmp.path());
        let err = open(tmp.path()).err().expect("refused");
        assert_eq!(
            err.to_string(),
            "a controller serves this state dir; set controller.socket"
        );
    }

    #[test]
    fn succeeds_when_no_controller_holds_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(open(tmp.path()).is_ok());
    }

    #[test]
    fn another_local_call_is_waited_on_then_refused_and_not_after_it_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let other = File::create(tmp.path().join("controller.lock")).unwrap();
        other.lock().unwrap();
        writeln!(&other, "4242").unwrap();
        let err = open(tmp.path()).err().expect("refused");
        assert!(matches!(err, StoreError::InUse), "{err}");
        drop(other);
        assert!(open(tmp.path()).is_ok());
    }

    #[test]
    fn a_stray_local_open_inside_the_controller_process_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let file = File::create(tmp.path().join("controller.lock")).unwrap();
        file.lock().unwrap();
        writeln!(&file, "{}\n/run/moor.sock", std::process::id()).unwrap();
        let err = open(tmp.path()).err().expect("refused");
        assert!(matches!(err, StoreError::Served), "{err}");
    }
}
