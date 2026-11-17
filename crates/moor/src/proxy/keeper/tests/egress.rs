//! A Docker Box's keeper reads the hosts the Box reached and writes each one into an event once.

use std::sync::{Arc, Mutex};

use moor_core::domain::egress::SEEN_FILE;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::Driver;
use moor_core::{BoxId, BoxPath};
use serde_json::{json, Value};

use super::super::egress::READ_EVERY;
use super::super::{Deps, Kept};
use super::ready_box;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;

struct Rig {
    _dir: tempfile::TempDir,
    store: Store,
    driver: Arc<FakeDriver>,
    clock: Arc<FakeClock>,
    id: BoxId,
    handle: String,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
        let driver = Arc::new(FakeDriver::new().machine_handles());
        let id = ready_box(&store, &driver);
        store.reserve(id, &FakeRng::default()).unwrap();
        let row = store.box_row(id).unwrap().unwrap();
        let handle = row.hands.unwrap().handle.unwrap().as_str().to_owned();
        Self {
            _dir: dir,
            store,
            driver,
            clock,
            id,
            handle,
        }
    }

    fn kept(&self) -> Kept {
        let deps = Deps {
            store: self.store.clone(),
            driver: Arc::clone(&self.driver) as Arc<dyn Driver>,
            clock: self.clock.clone(),
            log: Arc::new(Mutex::new(Vec::<u8>::new())),
        };
        Kept::new(Arc::new(deps), self.id, 41_000)
    }

    /// The Box's file of rows now holds `rows`.
    fn box_has(&self, rows: &Value) {
        let path = BoxPath::restore(SEEN_FILE).unwrap();
        self.driver
            .set_peek_text(&self.handle, &path, &rows.to_string());
    }

    /// The hosts of each `egress_seen` event, oldest first.
    fn seen(&self) -> Vec<Vec<String>> {
        self.store
            .events(self.id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "egress_seen")
            .map(|e| {
                e.payload["hosts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| format!("{} {} {}", h["host"], h["port"], h["path"]))
                    .collect()
            })
            .collect()
    }
}

fn row(host: &str, port: u16, path: &str, count: u64) -> Value {
    json!({"host": host, "port": port, "path": path, "first_seen": 1_000, "count": count})
}

#[test]
fn each_host_the_box_reached_is_written_once() {
    let rig = Rig::new();
    let kept = rig.kept();
    rig.box_has(&json!([
        row("example.com", 443, "proxied", 1),
        row("93.184.215.14", 443, "direct", 1),
    ]));
    assert!(kept.tick());
    assert_eq!(
        rig.seen(),
        [[
            r#""example.com" 443 "proxied""#,
            r#""93.184.215.14" 443 "direct""#
        ]]
    );

    rig.clock.sleep(READ_EVERY);
    assert!(kept.tick());
    assert_eq!(rig.seen().len(), 1, "nothing new, so no event");

    rig.box_has(&json!([
        row("example.com", 443, "proxied", 1),
        row("93.184.215.14", 443, "direct", 1),
        row("api.anthropic.com", 443, "credential", 4),
    ]));
    assert!(kept.tick());
    assert_eq!(rig.seen().len(), 1, "the next read is not due yet");
    rig.clock.sleep(READ_EVERY);
    assert!(kept.tick());
    assert_eq!(
        rig.seen().last().unwrap(),
        &[r#""api.anthropic.com" 443 "credential""#]
    );
}

#[test]
fn a_box_with_no_rows_writes_no_event() {
    let rig = Rig::new();
    assert!(rig.kept().tick());
    assert!(rig.seen().is_empty());
}
