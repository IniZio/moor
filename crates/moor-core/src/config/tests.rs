//! Tests for strict parsing and `ConfigError`.

use super::{parse_strict, ConfigError};

#[derive(Debug, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    #[serde(rename = "box")]
    box_: Option<ProbeBox>,
    egress: Option<Vec<String>>,
}

#[derive(Debug, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeBox {
    provider: Option<String>,
    enforce_egress: Option<bool>,
    max_boxes: Option<u32>,
}

#[test]
fn parse_strict_error_paths() {
    let cases = [
        ("box: {providr: ssh}", "box.providr", "unknown field"),
        ("egres: [a]", "egres", "unknown field"),
        (
            "box: {enforce_egress: \"yes\"}",
            "box.enforce_egress",
            "invalid boolean",
        ),
        ("egress: [a, 1, c]", "egress[1]", "must be quoted"),
        ("box: [unclosed", "", "invalid yaml"),
        ("- a", "", "expected mapping"),
    ];
    for (doc, path, why) in cases {
        let e = parse_strict::<Probe>("f.yaml", doc)
            .err()
            .unwrap_or_else(|| ConfigError::new("<ok>", "parsed"));
        assert_eq!(e.path, path, "{doc:?}: {e}");
        assert_eq!(e.file, "f.yaml");
        assert!(e.reason.contains(why), "{doc:?}: {e}");
    }
}

#[test]
fn parse_strict_empty_is_default() {
    for doc in ["", "  \n", "null", "~", "{}"] {
        assert_eq!(
            parse_strict::<Probe>("f", doc),
            Ok(Probe::default()),
            "{doc:?}"
        );
    }
}

#[test]
fn config_error_display() {
    let e = ConfigError::new("a.b", "bad");
    assert_eq!(e.to_string(), "a.b: bad");
    assert_eq!(e.in_file("f.yaml").to_string(), "f.yaml:a.b: bad");
    assert_eq!(
        ConfigError::new("", "bad").in_file("f").to_string(),
        "f: bad"
    );
    assert_eq!(ConfigError::new("", "bad").to_string(), "bad");
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    name: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Outer {
    a: Option<Item>,
    items: Option<Vec<Item>>,
}

#[test]
fn strict_parse_reads_known_fields() {
    let o = parse_strict::<Outer>("f.yaml", "a: {name: x}\nitems: [{name: y}, {}]").unwrap();
    assert_eq!(o.a.unwrap().name.as_deref(), Some("x"));
    let names: Vec<_> = o.items.unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, [Some("y".to_string()), None]);
}

#[test]
fn unknown_field_path_includes_the_key() {
    let cases = [
        ("zzz: 1", "zzz"),
        ("a: {zzz: 1}", "a.zzz"),
        ("items: [{}, {}, {zzz: 1}]", "items[2].zzz"),
    ];
    for (doc, path) in cases {
        let e = parse_strict::<Outer>("f.yaml", doc).unwrap_err();
        assert_eq!(e.path, path, "{doc:?}: {e}");
        assert!(e.reason.starts_with("unknown field `zzz`"), "{doc:?}: {e}");
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct Scalars {
    n: Option<u64>,
    s: Option<String>,
}

#[test]
fn unquoted_leading_zero_asks_for_quotes() {
    let e = parse_strict::<Scalars>("f", "s: 0755").unwrap_err();
    assert_eq!(e.path, "s");
    assert!(e.reason.contains("[0755] must be quoted"), "{e}");
    for doc in ["s: \"0755\"", "s: '0755'"] {
        let got = parse_strict::<Scalars>("f", doc).unwrap();
        assert_eq!(got.s.as_deref(), Some("0755"), "{doc}");
    }
    for doc in ["n: 0755", "n: 010"] {
        let e = parse_strict::<Scalars>("f", doc).unwrap_err();
        assert_eq!(e.path, "n", "{doc}: {e}");
    }
    assert_eq!(parse_strict::<Scalars>("f", "n: 755").unwrap().n, Some(755));
}

#[test]
fn invalid_yaml_reason_is_one_line() {
    let e = parse_strict::<Probe>("f.yaml", "box: [unclosed").unwrap_err();
    assert!(e.reason.starts_with("invalid yaml: "), "{e}");
    assert!(!e.reason.contains('\n'), "{e}");
}

#[test]
fn typed_error_reason_has_no_location() {
    let e = parse_strict::<Probe>("f.yaml", "box: {max_boxes: many}").expect_err("typed error");
    assert_eq!(e.path, "box.max_boxes");
    assert!(!e.reason.contains(" at line"), "{}", e.reason);
}
