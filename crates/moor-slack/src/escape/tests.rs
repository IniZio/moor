//! Tests for the escaping of Slack markup.

use super::escape;

#[test]
fn an_ampersand_is_escaped() {
    assert_eq!(escape("a & b"), "a &amp; b");
}

#[test]
fn a_less_than_sign_is_escaped() {
    assert_eq!(escape("a < b"), "a &lt; b");
}

#[test]
fn a_greater_than_sign_is_escaped() {
    assert_eq!(escape("a > b"), "a &gt; b");
}

#[test]
fn a_broadcast_with_an_ampersand_is_escaped_once() {
    assert_eq!(
        escape("<!channel> ship it & go"),
        "&lt;!channel&gt; ship it &amp; go"
    );
}

#[test]
fn text_without_markup_is_unchanged() {
    assert_eq!(escape("plain text"), "plain text");
}
