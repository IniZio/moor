//! Tests for the Report name checker and index parser.

use super::{parse_index, valid_name, Entry};

#[test]
fn names_accept_plain_markdown_files() {
    assert!(valid_name("plan.md"));
    assert!(valid_name("a-b_c.1.md"));
    assert!(valid_name("x.md.md"));
    assert!(valid_name("1.md"));
}

#[test]
fn names_reject_unsafe_or_empty_stems() {
    for bad in [
        ".md", "", "plan", "plan.txt", "a b.md", "a/b.md", "é.md", "a\n.md", "x.md.bak", "..md",
        "...md", ".md.md", ".a.md", "_a.md", "-a.md",
    ] {
        assert!(!valid_name(bad), "{bad:?}");
    }
}

#[test]
fn index_is_sorted_by_name() {
    let (entries, bad) = parse_index("b.md\t2\na.md\t10\n");
    assert_eq!(bad, 0);
    assert_eq!(
        entries,
        vec![
            Entry {
                name: "a.md".into(),
                size: 10
            },
            Entry {
                name: "b.md".into(),
                size: 2
            },
        ]
    );
}

#[test]
fn index_counts_malformed_lines_and_skips_blanks() {
    let (entries, bad) = parse_index("\nnotab\nx.md\tabc\nok.md\t5\n");
    assert_eq!(bad, 2);
    assert_eq!(entries.len(), 1);
}

#[test]
fn empty_index_has_no_entries() {
    assert_eq!(parse_index(""), (Vec::new(), 0));
}
