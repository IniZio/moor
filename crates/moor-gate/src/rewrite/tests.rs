#![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use moor_core::user::{Email, Identity};

use super::Splitter;
use crate::Person;

fn alice() -> Person {
    Person {
        identity: Identity::parse("authgear:alice").unwrap(),
        email: Email::parse("alice@example.com").unwrap(),
    }
}

#[test]
fn raw_bytes_after_a_bytes_frame_pass_untouched_across_chunks() {
    let stream = b"{\"frame\":\"bytes\",\"id\":\"a\",\"len\":5}\nx\ny\nz{\"frame\":\"resize\",\"id\":\"a\",\"cols\":80,\"rows\":24}\n";
    let (mut split, mut up, mut back) = (Splitter::default(), Vec::new(), Vec::new());
    for chunk in stream.chunks(3) {
        split.feed(chunk, &alice(), &mut up, &mut back).unwrap();
    }
    assert!(back.is_empty(), "{back:?}");
    assert_eq!(
        String::from_utf8(up).unwrap(),
        String::from_utf8(stream.to_vec()).unwrap()
    );
}

#[test]
fn a_malformed_line_is_answered_by_the_gate_and_not_sent_on() {
    let (mut split, mut up, mut back) = (Splitter::default(), Vec::new(), Vec::new());
    split
        .feed(b"{\"frame\":\"nope\"}\n", &alice(), &mut up, &mut back)
        .unwrap();
    assert!(up.is_empty());
    assert!(back[0].contains("malformed"), "{back:?}");
}
