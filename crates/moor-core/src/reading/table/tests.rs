#![allow(clippy::indexing_slicing, reason = "test assertions")]

use sha2::{Digest, Sha256};

use super::{IntentName, IntentTable, Request, TableError, INTENTS_TEXT};

/// Each shipped version's hash: a change to intents.yaml without a version bump fails here.
const SHIPPED: &[(u32, &str)] = &[(
    1,
    "3eb22b20e5e5c255116d0aba53dc160a8acb83bea2979d0140d1db9e1521a4b9",
)];

fn table(rows: &str) -> Result<IntentTable, TableError> {
    IntentTable::load(&format!("version: 1\nintents:\n{rows}"))
}

const COMMENT_ROW: &str = r#"  - name: comment
    words: comment on a pull request
    do:
      api.github.com:
        - "POST /repos/{repo}/issues/*/comments"
    examples:
      match:
        - "POST https://api.github.com/repos/acme/web/issues/12/comments"
      no_match:
        - "POST https://api.github.com/repos/acme/other/issues/12/comments"
"#;

#[test]
fn the_shipped_table_loads_with_its_comment_row() {
    let t = IntentTable::shipped().unwrap();
    let row = t.row(&IntentName::parse("comment").unwrap()).unwrap();
    assert!(row.app);
    let lines: Vec<_> = row
        .lines
        .iter()
        .map(|l| (l.host.as_str(), l.entry.as_str()))
        .collect();
    assert_eq!(
        lines,
        [
            ("api.github.com", "POST /repos/{repo}/issues/*/comments"),
            ("api.github.com", "PATCH /repos/{repo}/issues/comments/*"),
            ("api.github.com", "GET /repos/{repo}/issues/*/comments"),
        ]
    );
    assert!(matches!(&row.never[..], [Request::Http { method, .. }] if method == "POST"));
}

#[test]
fn every_change_to_the_table_bumps_its_version() {
    let t = IntentTable::shipped().unwrap();
    let hash = format!("{:x}", Sha256::digest(INTENTS_TEXT.as_bytes()));
    let kept = SHIPPED.iter().find(|(v, _)| *v == t.version.0);
    assert_eq!(
        kept.map(|(_, h)| *h),
        Some(hash.as_str()),
        "bump version and keep its hash"
    );
}

#[test]
fn a_table_whose_example_fails_is_refused_naming_the_row() {
    let bad = COMMENT_ROW.replace("acme/other/issues", "acme/web/issues");
    let e = table(&bad).unwrap_err();
    assert_eq!(e.row.as_deref(), Some("comment"));
    assert!(e.to_string().contains("row `comment`"), "{e}");
    assert!(e.why.contains("a do line lets it through"), "{e}");
    let unmatched = COMMENT_ROW.replace(
        "issues/12/comments\"\n      no_match",
        "issues/12/reactions\"\n      no_match",
    );
    assert!(table(&unmatched)
        .unwrap_err()
        .why
        .contains("no do line lets it through"));
    assert!(table(COMMENT_ROW).is_ok());
}

#[test]
fn a_row_line_takes_one_method_on_the_box_repo_and_a_named_path() {
    for line in [
        "GET|POST /repos/{repo}/issues/*/comments",
        "POST /repos/{repo}/**",
        "GET /repos/{any_repo}/issues",
        "POST /rate_limit",
        "*",
    ] {
        let row = COMMENT_ROW.replace(
            "POST /repos/{repo}/issues/*/comments\"\n    examples",
            &format!("{line}\"\n    examples"),
        );
        let e = table(&row).unwrap_err();
        assert_eq!(e.row.as_deref(), Some("comment"), "{line}");
    }
}

#[test]
fn names_are_lower_case_words_and_unique() {
    assert!(IntentName::parse("pr-open").is_some());
    for bad in ["Comment", "pr_open", "-pr", "pr--open", ""] {
        assert!(IntentName::parse(bad).is_none(), "{bad}");
    }
    let twice = format!("{COMMENT_ROW}{COMMENT_ROW}");
    assert!(table(&twice).unwrap_err().why.contains("earlier row"));
}

#[test]
fn words_are_one_short_line() {
    let long = COMMENT_ROW.replace("comment on a pull request", &"w".repeat(121));
    assert!(table(&long).unwrap_err().why.contains("one line"));
}

#[test]
fn the_reader_list_has_one_line_a_row() {
    let t = IntentTable::shipped().unwrap();
    assert_eq!(t.list().lines().count(), t.intents.len());
    assert!(t.list().starts_with("- comment: comment on"));
}
