use super::{numeric, Value};

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
