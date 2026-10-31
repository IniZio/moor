//! The runner tells each linked machine's agent which Boxes are open, and the agent removes
//! the folders of the rest.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::rig::Rig;

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn make(home: &Path, id: i64) {
    for root in ["moor", "moor-state"] {
        let dir = home.join(format!("{root}/{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file"), "x").unwrap();
    }
}

fn exists(home: &Path, id: i64) -> bool {
    home.join(format!("moor/{id}")).exists() || home.join(format!("moor-state/{id}")).exists()
}

#[test]
fn a_machine_loses_the_folders_of_boxes_the_controller_no_longer_lists_as_open() {
    let rig = Rig::with_open(&[7, 12]);
    make(&rig.remote, 9);
    make(&rig.remote, 12);
    eventually("Box 9's folders are swept", || !exists(&rig.remote, 9));
    assert!(rig.remote.join("moor/7").is_dir(), "an open Box");
    assert!(exists(&rig.remote, 12), "an open Box, listed beside Box 7");
    rig.open.lock().unwrap().retain(|id| *id != 12);
    eventually("Box 12's folders are swept once it closes", || {
        !exists(&rig.remote, 12)
    });
    assert!(rig.remote.join("moor/7").is_dir(), "still open");
}

#[test]
fn nothing_is_swept_while_the_controller_cannot_list_the_open_boxes() {
    let rig = Rig::listing_fails();
    let deadline = Instant::now() + Duration::from_secs(20);
    while rig.shows_refused.load(Ordering::SeqCst) < 6 {
        assert!(Instant::now() < deadline, "the runner never asked");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(exists(&rig.remote, 9), "swept with no list of open Boxes");
    assert!(rig.remote.join("moor/7").is_dir(), "swept an open Box");
    rig.show_fails.store(false, Ordering::SeqCst);
    eventually("Box 9's folders are swept once the list is back", || {
        !exists(&rig.remote, 9)
    });
    assert!(rig.remote.join("moor/7").is_dir(), "an open Box");
}
