use std::sync::{Arc, Mutex};

use moor_core::config::UserConfig;
use moor_core::seam::driver::DriverError;

use super::{req, world};
use crate::cx::Cx;
use crate::ops::common::reconcile;
use crate::ops::start::run;
use crate::testing::{box_name, driver::FakeDriver};

#[test]
fn the_box_events_carry_what_the_provider_reports_while_the_acquire_runs() {
    let w = world(FakeDriver::new());
    let store = w.cx.store().unwrap().clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let during = Arc::clone(&seen);
    w.driver.notes_while_acquiring(
        &["machine created", "waiting for SSH: port 22 closed"],
        move || {
            let id = store
                .open_box_by_name(&box_name("demo"))
                .unwrap()
                .unwrap()
                .id;
            let texts = store
                .events(id, 50)
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "box_progress")
                .filter_map(|e| e.payload.get("text")?.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            *during.lock().unwrap() = texts;
        },
    );
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        ["machine created", "waiting for SSH: port 22 closed"]
    );
}

#[test]
fn a_failing_box_list_shows_once_on_a_starting_box_in_plain_words() {
    let w = world(FakeDriver::new());
    let store = w.cx.store().unwrap().clone();
    let (dir, driver, surface, proc) = (
        w.dir.path().to_owned(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let during = Arc::clone(&seen);
    w.driver
        .notes_while_acquiring(&["machine created"], move || {
            driver.fail_list_always(DriverError::Failed {
                summary:
                    "box list failed (crabbox exit 2): provider=external refuses it token=ab12CD34"
                        .to_owned(),
            });
            let settle = Cx::fake_with(
                &dir,
                driver.clone(),
                surface.clone(),
                proc.clone(),
                UserConfig::default(),
            );
            reconcile::run(&settle);
            reconcile::run(&settle);
            let id = store
                .open_box_by_name(&box_name("demo"))
                .unwrap()
                .unwrap()
                .id;
            *during.lock().unwrap() = store
                .events(id, 50)
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "box_progress")
                .filter_map(|e| e.payload.get("text")?.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
        });
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "machine created",
            "box list failed (crabbox exit 2): provider=external refuses it …"
        ]
    );
}
