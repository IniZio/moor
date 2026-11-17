//! Tests for the composition root.

use super::*;

fn window(value: Option<&str>) -> Duration {
    let pairs: Vec<(&str, &str)> = value.map(|v| (PROMPT_WINDOW_ENV, v)).into_iter().collect();
    prompt_window(&Env::from_pairs(pairs))
}

#[test]
fn a_built_context_carries_the_prompt_window_from_the_environment() {
    let env = Env::from_pairs([(PROMPT_WINDOW_ENV, "0")]);
    assert_eq!(build(&env).unwrap().prompt_window(), Duration::ZERO);
}

#[test]
fn the_prompt_window_defaults_to_eight_seconds() {
    assert_eq!(window(None), Duration::from_secs(8));
    assert_eq!(window(Some("soon")), Duration::from_secs(8));
}

#[test]
fn the_prompt_window_reads_milliseconds_from_the_environment() {
    assert_eq!(window(Some("250")), Duration::from_millis(250));
    assert_eq!(window(Some(" 0 ")), Duration::ZERO);
}

#[test]
fn store_migrate_migrates_and_warns_on_a_store_a_newer_build_migrated() {
    use crate::store::db::testing::open_conn;
    use crate::store::db::DbExt as _;
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([("HOME", dir.path().to_str().unwrap())]).with_state_dir(dir.path());
    let line = migrate_store(&env).unwrap();
    assert!(line.starts_with("migrated the store in "), "{line}");
    open_conn(&dir.path().join("moor.db"))
        .run(
            sea_query::Query::insert()
                .into_table("refinery_schema_history")
                .columns(["version", "name", "applied_on", "checksum"])
                .values_panic([
                    10_000.into(),
                    "future".into(),
                    "2030-01-01T00:00:00Z".into(),
                    "1".into(),
                ]),
        )
        .unwrap();
    let line = migrate_store(&env).unwrap();
    assert!(
        line.starts_with("warning: the state database is at migration 10000"),
        "{line}"
    );
}
