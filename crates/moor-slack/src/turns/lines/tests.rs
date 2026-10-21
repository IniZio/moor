//! Tests for the lines a turn ends with.

use moor_core::secret::Secret;

use super::{redact, reply, stopped};

#[test]
fn a_reply_escapes_broadcasts_mentions_and_links() {
    let got = reply("<!channel> ship it & tell <@U9> about <https://x|this>");
    let want = "&lt;!channel&gt; ship it &amp; tell &lt;@U9&gt; about &lt;https://x|this&gt;";
    assert_eq!(got, want);
    assert_eq!(
        stopped("rate <limited>"),
        "The coordinator stopped: rate &lt;limited&gt;."
    );
}

#[test]
fn a_long_reply_is_cut_at_4000_characters_with_a_marker() {
    let got = reply(&"é".repeat(5000));
    assert_eq!(got.chars().count(), 4000);
    assert!(got.ends_with("é… (cut)"), "{got}");
}

#[test]
fn a_reason_hides_the_credential() {
    let token = Secret::new(b"sk-ant-oat01-s3cr3t".to_vec());
    assert_eq!(
        redact("bad sk-ant-oat01-s3cr3t here", &token),
        "bad [token] here"
    );
}
