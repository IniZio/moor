use rusqlite::types::Value;

use super::{numeric, placeholders};

#[test]
fn bare_placeholders_are_numbered_in_order() {
    assert_eq!(
        placeholders("INSERT INTO t (a, b) VALUES (?, ?)"),
        "INSERT INTO t (a, b) VALUES ($1, $2)"
    );
}

#[test]
fn numbered_placeholders_keep_their_number_and_may_repeat() {
    assert_eq!(
        placeholders("SELECT ?2 WHERE a = ?1 OR b = ?1 OR c = ?10"),
        "SELECT $2 WHERE a = $1 OR b = $1 OR c = $10"
    );
}

#[test]
fn a_bare_placeholder_follows_the_largest_number_so_far() {
    assert_eq!(placeholders("?3, ?, ?1, ?"), "$3, $4, $1, $5");
}

#[test]
fn question_marks_in_quoted_text_names_and_comments_stay() {
    let sql = "SELECT '?', 'it''s ?', \"a?\" -- ?\n, ? /* ? */ , ?";
    assert_eq!(
        placeholders(sql),
        "SELECT '?', 'it''s ?', \"a?\" -- ?\n, $1 /* ? */ , $2"
    );
}

#[test]
fn a_block_comment_ends_only_at_its_own_close() {
    assert_eq!(placeholders("/*/ ? */ ?"), "/*/ ? */ $1");
}

#[test]
fn unterminated_text_is_copied_to_the_end() {
    assert_eq!(placeholders("SELECT ?, 'open ?"), "SELECT $1, 'open ?");
    assert_eq!(placeholders("? -- no newline ?"), "$1 -- no newline ?");
}

#[test]
fn a_single_dash_or_slash_is_not_a_comment() {
    assert_eq!(placeholders("a - ? / ?"), "a - $1 / $2");
}

/// A binary numeric of base-10000 `digits` at `weight`.
fn raw(weight: i16, negative: bool, scale: u16, digits: &[i16]) -> Vec<u8> {
    let sign: u16 = if negative { 0x4000 } else { 0 };
    let len = i16::try_from(digits.len()).unwrap();
    let mut out = Vec::new();
    for w in [len, weight, sign.cast_signed(), scale.cast_signed()]
        .iter()
        .chain(digits)
    {
        out.extend(w.to_be_bytes());
    }
    out
}

#[test]
fn a_whole_numeric_reads_as_an_integer() {
    assert_eq!(numeric(&raw(0, false, 0, &[])).unwrap(), Value::Integer(0));
    assert_eq!(
        numeric(&raw(0, false, 0, &[42])).unwrap(),
        Value::Integer(42)
    );
    assert_eq!(
        numeric(&raw(1, true, 0, &[12, 3456])).unwrap(),
        Value::Integer(-123_456)
    );
    assert_eq!(
        numeric(&raw(2, false, 0, &[7])).unwrap(),
        Value::Integer(700_000_000)
    );
    assert_eq!(
        numeric(&raw(4, false, 0, &[922, 3372, 368, 5477, 5807])).unwrap(),
        Value::Integer(i64::MAX)
    );
}

#[test]
fn a_fractional_or_huge_numeric_reads_as_a_real() {
    assert_eq!(
        numeric(&raw(0, false, 1, &[1, 5000])).unwrap(),
        Value::Real(1.5)
    );
    assert_eq!(
        numeric(&raw(-1, true, 2, &[2500])).unwrap(),
        Value::Real(-0.25)
    );
    assert_eq!(numeric(&raw(5, false, 0, &[1])).unwrap(), Value::Real(1e20));
}

#[test]
fn a_nan_or_short_numeric_is_refused() {
    let mut nan = raw(0, false, 0, &[]);
    if let Some(sign) = nan.get_mut(4) {
        *sign = 0xC0;
    }
    assert!(numeric(&nan).is_err());
    let short = raw(0, false, 0, &[1]);
    assert!(numeric(short.get(..9).unwrap()).is_err());
}
