use super::{doing_line, DOING_MAX};

#[test]
fn a_line_keeps_only_its_first_non_blank_line_squeezed() {
    assert_eq!(
        doing_line("\n  Bash:   cargo \t test\nsecond line").as_deref(),
        Some("Bash: cargo test")
    );
}

#[test]
fn nothing_left_is_none() {
    assert_eq!(doing_line(" \n\t"), None);
    assert_eq!(doing_line(""), None);
}

#[test]
fn a_long_line_is_cut_to_the_cap_with_dots() {
    let line = doing_line(&"a".repeat(500)).unwrap();
    assert_eq!(line.chars().count(), DOING_MAX);
    assert!(line.ends_with("..."));
}

#[test]
fn a_line_at_the_cap_is_kept_whole() {
    let exact = "b".repeat(DOING_MAX);
    assert_eq!(doing_line(&exact).as_deref(), Some(exact.as_str()));
}

#[test]
fn the_cut_counts_characters_not_bytes() {
    let line = doing_line(&"é".repeat(200)).unwrap();
    assert_eq!(line.chars().count(), DOING_MAX);
}
