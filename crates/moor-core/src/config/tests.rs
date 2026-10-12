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
}

#[test]
fn parse_strict_error_paths() {
    let cases = [
        ("box: {providr: ssh}", "box.providr", "unknown field"),
        ("egres: [a]", "egres", "unknown field"),
        (
            "box: {enforce_egress: \"yes\"}",
            "box.enforce_egress",
            "invalid type",
        ),
        ("egress: [a, 1, c]", "egress[1]", "invalid type"),
        ("box: [unclosed", "", "invalid yaml"),
        ("- a", "", "invalid type"),
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
