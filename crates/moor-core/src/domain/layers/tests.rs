#![allow(clippy::indexing_slicing, reason = "test assertions")]

use super::{
    chain, checkpoint_after, failed_layer, nearest, LayerIdentity, LayerKey, WarmDisk,
    IGNORED_NAMED,
};
use crate::user::UserId;

const U: UserId = UserId::new(1);

const ID: LayerIdentity<'static> = LayerIdentity {
    user: U,
    repo: "o/n",
    cache: "",
    class: "",
    pinned_base: None,
};

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn the_same_identity_and_lines_give_the_same_keys() {
    let a = chain(&ID, &lines(&["npm ci", "make"]));
    let b = chain(&ID, &lines(&["npm ci", "make"]));
    assert_eq!(a, b);
    assert_eq!(a.len(), 3, "the clone layer and one per line");
}

#[test]
fn a_changed_line_invalidates_itself_and_every_later_layer_only() {
    let before = chain(&ID, &lines(&["a", "b", "c"]));
    let after = chain(&ID, &lines(&["a", "B", "c"]));
    assert_eq!(before[..2], after[..2]);
    assert_ne!(before[2], after[2]);
    assert_ne!(before[3], after[3]);
}

#[test]
fn each_part_of_the_identity_changes_every_layer() {
    let one = chain(&ID, &lines(&["a"]));
    for other in [
        LayerIdentity {
            user: UserId::new(2),
            ..ID
        },
        LayerIdentity { repo: "o/m", ..ID },
        LayerIdentity {
            cache: "npm ~/.npm",
            ..ID
        },
        LayerIdentity {
            class: "large",
            ..ID
        },
        LayerIdentity {
            pinned_base: Some("abc"),
            ..ID
        },
    ] {
        let keys = chain(&other, &lines(&["a"]));
        assert!(one.iter().zip(&keys).all(|(x, y)| x != y), "{other:?}");
    }
}

#[test]
fn a_repo_that_pins_its_base_gets_a_chain_per_commit() {
    let at = |base| {
        chain(
            &LayerIdentity {
                pinned_base: Some(base),
                ..ID
            },
            &lines(&["a"]),
        )
    };
    assert_eq!(at("abc"), at("abc"));
    assert_ne!(at("abc")[1], at("def")[1]);
}

#[test]
fn the_keys_hold_no_commit_and_match_their_definition() {
    let keys = chain(&ID, &lines(&["a"]));
    let clone = LayerKey::of(&["clone", "1", "o/n", "", ""]);
    assert_eq!(keys[0], clone);
    assert_eq!(keys[1], LayerKey::of(&["line", clone.as_str(), "a"]));
}

#[test]
fn a_key_from_when_the_base_was_in_it_is_never_asked_for() {
    let then = LayerKey::of(&["clone", "1", "o/n", "abc"]);
    let then_cached = LayerKey::of(&["clone", "1", "o/n", "abc", "npm ~/.npm"]);
    for now in [
        ID,
        LayerIdentity {
            pinned_base: Some("abc"),
            ..ID
        },
        LayerIdentity {
            cache: "npm ~/.npm",
            pinned_base: Some("abc"),
            ..ID
        },
    ] {
        let key = &chain(&now, &[])[0];
        assert!(*key != then && *key != then_cached, "{now:?}");
    }
}

#[test]
fn a_line_cannot_pose_as_the_clone_or_shift_text_between_parts() {
    let split = chain(&LayerIdentity { repo: "o/n", ..ID }, &lines(&["c"]));
    let joined = chain(&LayerIdentity { repo: "o/nc", ..ID }, &[]);
    assert_ne!(split[1], joined[0]);
    assert_ne!(
        chain(
            &LayerIdentity {
                cache: "n",
                ..LayerIdentity { repo: "o/", ..ID }
            },
            &[]
        )[0],
        chain(&LayerIdentity { repo: "o/n", ..ID }, &[])[0],
        "the repo and cache are kept apart"
    );
}

fn disk(changed: Option<u64>, ignored: &[&str]) -> WarmDisk {
    WarmDisk {
        commit: "0123456789abcdef0123".to_owned(),
        changed,
        ignored: lines(ignored),
    }
}

#[test]
fn the_note_names_the_disk_s_commit_its_changes_and_its_ignored_paths() {
    let note = disk(Some(3), &["dist/", "target/"]).note();
    assert!(note.contains("commit 0123456789ab,"), "{note}");
    assert!(note.contains("3 tracked paths changed"), "{note}");
    assert!(note.contains("dist/, target/."), "{note}");
    assert!(note.contains("may have skipped rebuilding them"), "{note}");
}

#[test]
fn the_note_says_when_it_could_not_count_and_warns_only_of_paths_that_exist() {
    let note = disk(None, &[]).note();
    assert!(
        note.contains("An unknown number of tracked paths"),
        "{note}"
    );
    assert!(!note.contains("skipped rebuilding"), "{note}");
    assert!(disk(Some(1), &[]).note().contains("1 tracked path changed"));
}

#[test]
fn the_note_caps_the_paths_it_names() {
    let many: Vec<String> = (0..=IGNORED_NAMED).map(|i| format!("p{i}/")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let note = disk(Some(1), &refs).note();
    assert!(note.contains(", and more."), "{note}");
    assert!(!note.contains(&format!("p{IGNORED_NAMED}/")), "{note}");
}

#[test]
fn keys_are_hex_sha256() {
    let key = &chain(&ID, &[])[0];
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
    let keys = chain(&ID, &lines(&["a", "b", "c", "d"]));
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

#[test]
fn the_finished_key_changes_with_what_it_holds() {
    const STEPS: super::FinishedSteps<'static> = super::FinishedSteps {
        install: "i",
        profile_sha: "s",
        plugins: "p",
        sources: "o",
        env: "e",
        dotfiles: "d",
    };
    let top = &chain(&ID, &lines(&["a"]))[1];
    let key = super::finished_key(top, &STEPS);
    assert_eq!(key, super::finished_key(top, &STEPS));
    let other = &chain(&ID, &lines(&["b"]))[1];
    assert_ne!(key, super::finished_key(other, &STEPS));
    for changed in [
        super::FinishedSteps {
            install: "x",
            ..STEPS
        },
        super::FinishedSteps {
            profile_sha: "x",
            ..STEPS
        },
        super::FinishedSteps {
            plugins: "x",
            ..STEPS
        },
        super::FinishedSteps {
            sources: "x",
            ..STEPS
        },
        super::FinishedSteps { env: "x", ..STEPS },
        super::FinishedSteps {
            dotfiles: "x",
            ..STEPS
        },
    ] {
        assert_ne!(key, super::finished_key(top, &changed), "{changed:?}");
    }
    let bob = LayerIdentity {
        user: UserId::new(2),
        ..ID
    };
    assert_ne!(
        key,
        super::finished_key(&chain(&bob, &lines(&["a"]))[1], &STEPS)
    );
}
