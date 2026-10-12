//! Tests for the JSON envelope fallback.

use super::fallback;

/// The fallback is byte-identical to the envelope the hand-built string produced.
#[test]
fn fallback_bytes_are_stable() {
    assert_eq!(
        fallback("a \"b\""),
        concat!(
            r#"{"schema":"moor/1","cmd":"a \"b\"","class":"failed","value":null,"warnings":[],"#,
            r#""error":{"kind":"internal","message":"response could not be serialised","#,
            r#""hints":[],"context":[]}}"#,
            "\n"
        )
    );
}
