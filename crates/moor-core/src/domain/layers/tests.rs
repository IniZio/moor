#![allow(clippy::indexing_slicing, reason = "test assertions")]

use super::{chain, checkpoint_after, failed_layer, nearest, LayerKey};

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn the_same_repo_base_and_lines_give_the_same_keys() {
    let a = chain("o/n", "abc", &lines(&["npm ci", "make"]));
    let b = chain("o/n", "abc", &lines(&["npm ci", "make"]));
    assert_eq!(a, b);
    assert_eq!(a.len(), 3, "the clone layer and one per line");
}

#[test]
fn a_changed_line_invalidates_itself_and_every_later_layer_only() {
    let before = chain("o/n", "abc", &lines(&["a", "b", "c"]));
    let after = chain("o/n", "abc", &lines(&["a", "B", "c"]));
    assert_eq!(before[..2], after[..2]);
    assert_ne!(before[2], after[2]);
    assert_ne!(before[3], after[3]);
}

#[test]
fn another_base_or_repo_changes_every_layer() {
    let one = chain("o/n", "abc", &lines(&["a"]));
    for other in [
        chain("o/n", "abc9", &lines(&["a"])),
        chain("o/m", "abc", &lines(&["a"])),
    ] {
        assert!(one.iter().zip(&other).all(|(x, y)| x != y));
    }
}

#[test]
fn a_line_cannot_pose_as_the_clone_or_shift_text_between_parts() {
    let split = chain("o/n", "ab", &lines(&["c"]));
    let joined = chain("o/n", "abc", &[]);
    assert_ne!(split[1], joined[0]);
    assert_ne!(
        chain("o", "n/abc", &[])[0],
        chain("o/n", "abc", &[])[0],
        "the repo and base are kept apart"
    );
}

#[test]
fn keys_are_hex_sha256() {
    let key = &chain("o/n", "abc", &[])[0];
    assert_eq!(key.as_str().len(), 64);
    assert!(key.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        LayerKey::restore(key.as_str().to_owned()).as_ref(),
        Some(key)
    );
    assert_eq!(LayerKey::restore("nope".to_owned()), None);
}

#[test]
fn the_clone_a_slow_line_and_the_last_line_are_checkpointed() {
    assert!(checkpoint_after(0, 0, 3), "the clone");
    assert!(checkpoint_after(2, 20_001, 3), "a line over 20 s");
    assert!(checkpoint_after(3, 0, 3), "the last line");
    assert!(!checkpoint_after(1, 20_000, 3), "a line of exactly 20 s");
    assert!(!checkpoint_after(2, 5, 3), "a cheap middle line");
}

#[test]
fn retry_resumes_from_the_deepest_checkpoint_at_or_below_the_failed_layer() {
    let keys = chain("o/n", "abc", &lines(&["a", "b", "c", "d"]));
    let held = [keys[0].clone(), keys[2].clone(), keys[4].clone()];
    let stored = |k: &LayerKey| held.contains(k);
    assert_eq!(nearest(&keys, 3, stored), Some(2));
    assert_eq!(nearest(&keys, 2, stored), Some(2));
    assert_eq!(nearest(&keys, 1, stored), Some(0));
    assert_eq!(
        nearest(&keys, 9, stored),
        Some(4),
        "past the top is the top"
    );
    assert_eq!(nearest(&keys, 3, |_| false), None);
}

#[test]
fn the_failed_layer_is_the_failed_line_s_position_else_the_top() {
    let ls = lines(&["a", "b", "b"]);
    assert_eq!(failed_layer(&ls, "a"), 1);
    assert_eq!(
        failed_layer(&ls, "b"),
        2,
        "a repeated line counts from its first"
    );
    assert_eq!(
        failed_layer(&ls, "o/m: x"),
        3,
        "a later repo's line ran after all of these"
    );
}
