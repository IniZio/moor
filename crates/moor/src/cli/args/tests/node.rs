//! `moor node set`.

use moor_core::request::{CapEdit, NodeSettingsSetReq};
use moor_core::Request;

use super::{req, run};

#[test]
fn node_set_asks_for_a_cap_or_no_cap_and_labels() {
    let one = std::num::NonZeroU32::new(1).unwrap();
    let want = |max_boxes, labels| {
        Request::NodeSettingsSet(NodeSettingsSetReq {
            identity: None,
            name: "box-host".to_owned(),
            max_boxes,
            labels,
            expected_revision: None,
        })
    };
    assert_eq!(
        req(&["node", "set", "box-host", "--max-boxes", "1"]),
        want(CapEdit::Set(one), None)
    );
    assert_eq!(
        req(&["node", "set", "box-host", "--no-cap"]),
        want(CapEdit::Clear, None)
    );
    assert_eq!(
        req(&["node", "set", "box-host", "--label", "gpu", "--label", "big"]),
        want(
            CapEdit::Keep,
            Some(vec!["gpu".to_owned(), "big".to_owned()])
        )
    );
    assert_eq!(
        req(&["node", "set", "box-host", "--no-labels"]),
        want(CapEdit::Keep, Some(Vec::new()))
    );
    for bad in [
        &["node", "set", "box-host"][..],
        &["node", "set", "box-host", "--max-boxes", "0"],
        &["node", "set", "box-host", "--max-boxes", "1", "--no-cap"],
        &["node", "set", "box-host", "--label", "a", "--no-labels"],
    ] {
        assert!(run(bad).is_err(), "{bad:?}");
    }
}
