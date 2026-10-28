//! Only `store/` may name `rusqlite`, so a second database backend stays a change to `store/`.

use std::path::{Path, PathBuf};

/// Files outside `store/` that already name `rusqlite`; move one onto `Store` methods and
/// delete it from here. A file that is not listed must not be added.
const LEGACY: &[&str] = &[
    "controller/tests/front_proxy.rs",
    "controller/tests/users.rs",
    "controller/watch/tests/parent.rs",
    "ops/close/tests/promote.rs",
    "ops/close/tests/transcript.rs",
    "ops/common/activity/tests.rs",
    "ops/host_session/tests.rs",
    "ops/open_session/tests.rs",
    "ops/run/tests.rs",
    "ops/sessions/tests.rs",
    "ops/show_session/tests.rs",
    "ops/show/tests/events.rs",
    "ops/start/tests/activity.rs",
    "ops/start/tests/others.rs",
    "ops/start/tests/relaunch.rs",
    "ops/start/tests/repo.rs",
    "ops/start/tests/transcript.rs",
    "ops/wait_events/tests.rs",
    "proxy/adopt/tests.rs",
    "proxy/detached/tests.rs",
    "proxy/detached/tests/stop.rs",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn naming_rusqlite_outside_store() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut named: Vec<String> = files
        .iter()
        .map(|f| f.strip_prefix(&src).unwrap().to_string_lossy().into_owned())
        .filter(|rel| !rel.starts_with("store/"))
        .filter(|rel| {
            std::fs::read_to_string(src.join(rel))
                .unwrap()
                .contains("rusqlite")
        })
        .collect();
    named.sort();
    named
}

#[test]
fn no_new_file_outside_store_names_rusqlite() {
    let named = naming_rusqlite_outside_store();
    let new: Vec<&String> = named
        .iter()
        .filter(|f| !LEGACY.contains(&f.as_str()))
        .collect();
    assert!(
        new.is_empty(),
        "go through Store methods instead of naming rusqlite in: {new:?}"
    );
}

#[test]
fn the_legacy_list_has_no_file_that_stopped_naming_rusqlite() {
    let named = naming_rusqlite_outside_store();
    let stale: Vec<&&str> = LEGACY
        .iter()
        .filter(|f| !named.iter().any(|n| n == *f))
        .collect();
    assert!(stale.is_empty(), "delete from LEGACY: {stale:?}");
}
