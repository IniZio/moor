use super::*;

fn key(n: i64) -> BoxKey {
    BoxKey::new(BoxId::new(n))
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

#[test]
fn slug_carries_id_and_nonce() {
    assert_eq!(key(7).slug("a1b2c3"), "moor-7-a1b2c3");
}

#[test]
fn pool_slug_carries_controller_and_id_and_parses_back() {
    let ctl = ControllerId::from_bytes([0x1a, 0x2b, 0x3c, 0x4d]);
    let slug = key(7).pool_slug(&ctl);
    assert_eq!(slug, "moor-1a2b3c4d-7");
    assert_eq!(parse_pool_slug(&slug), Some((ctl, BoxId::new(7))));
}

#[test]
fn controller_ids_are_zero_padded_lowercase_hex() {
    assert_eq!(
        ControllerId::from_bytes([0, 0, 0, 0xab]).as_str(),
        "000000ab"
    );
}

#[test]
fn only_a_well_formed_pool_slug_parses() {
    for bad in [
        "moor-7",
        "moor-1a2b3c4d",
        "moor-1a2b3c4d-",
        "moor-1a2b3c4d-0",
        "moor-1a2b3c4d-07",
        "moor-1a2b3c4d--7",
        "moor-1a2b3c4d-7-x",
        "moor-1A2B3C4D-7",
        "moor-1a2b3c4-7",
        "moor-1a2b3c4d5-7",
        "moor-1a2b3c4g-7",
        "box-1a2b3c4d-7",
        "../moor-1a2b3c4d-7",
        "moor-1a2b3c4d-7/x",
    ] {
        assert_eq!(parse_pool_slug(bad), None, "{bad}");
    }
}

#[test]
fn cred_and_tmp_dirs_carry_id_and_nonce() {
    assert_eq!(key(7).cred_dir("n"), ".moor-cred-7-n");
    assert_eq!(key(7).tmp_dir("n"), ".moor-tmp-7-n");
}

#[test]
fn branch_flattens_slashes() {
    assert_eq!(branch(&name("moor/7")), "moor/moor--7");
    assert_eq!(branch(&name("web/fix-bug")), "moor/web--fix-bug");
    assert_eq!(branch(&name("fix-bug")), "moor/fix-bug");
}

#[test]
fn branch_holds_exactly_one_slash() {
    for n in ["moor/7", "web/fix-bug", "fix-bug"] {
        assert_eq!(branch(&name(n)).matches('/').count(), 1, "{n}");
    }
}

#[test]
fn branch_is_one_to_one() {
    let branches = [
        branch(&name("a-b/c")),
        branch(&name("a/b-c")),
        branch(&name("a-b-c")),
    ];
    assert_eq!(branches, ["moor/a-b--c", "moor/a--b-c", "moor/a-b-c"]);
}

#[test]
fn slug_fits_crabbox_cap_for_any_id() {
    for id in [0, 1, 7, 123_456_789, i64::MAX] {
        let slug = key(id).slug("a1b2c3");
        assert!(slug.len() <= 41, "{slug}");
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{slug}"
        );
    }
}

#[test]
fn session_ref_carries_the_id() {
    assert_eq!(BoxKey::new(BoxId::new(7)).session_ref(), "moor-7");
}

#[test]
fn machine_dir_sits_under_the_work_root_by_id() {
    assert_eq!(key(7).machine_dir(), "moor/7");
}

#[test]
fn machine_state_dir_sits_beside_the_copy_by_id() {
    assert_eq!(key(7).machine_state_dir(), "moor-state/7");
}
