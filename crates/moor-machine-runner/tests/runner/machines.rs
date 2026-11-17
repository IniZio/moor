//! What the runner tells the controller of a machine that provides Boxes.

use crate::broker::Host;

#[test]
fn a_machine_that_provides_boxes_links_saying_how() {
    let host = Host::boxes();
    let links = host.rig.link.links.lock().unwrap().clone().unwrap();
    assert_eq!(links[0]["provides"], "fake", "{links:?}");
}
