//! `down`'s flag.

use super::{run, Action};

#[test]
fn down_refuses_open_boxes_unless_forced() {
    let (_, plain) = run(&["down"]).expect("parses");
    assert!(matches!(plain, Action::Down { force: false }));
    let (_, forced) = run(&["down", "--force"]).expect("parses");
    assert!(matches!(forced, Action::Down { force: true }));
}
