use super::snippet;

#[test]
fn the_snippet_matches_its_golden_copy() {
    assert_eq!(snippet(), include_str!("snippet.golden"));
}
