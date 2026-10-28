use super::snippet;

#[test]
fn the_snippet_matches_its_golden_copy() {
    assert_eq!(
        snippet("/opt/moor/herdr/bin/shell.sh"),
        include_str!("snippet.golden")
    );
}
