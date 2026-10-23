//! Tests for the lines a turn ends with.

use moor_core::secret::Secret;

use super::{plugin, redact, reply, stopped};

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

#[test]
fn the_plugin_line_names_the_plugin_without_letting_markup_through() {
    let line = plugin("a<!here>`b&");
    assert_eq!(
        line,
        "The coordinator did not start: Claude Code loaded the plugin `a&lt;!here&gt;'b&amp;`, which is not built in."
    );
    let long = plugin(&"x".repeat(200));
    assert!(long.contains(&format!("`{}`", "x".repeat(80))), "{long}");
}
