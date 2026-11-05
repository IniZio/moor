//! The controller's own egress key.

use std::os::unix::fs::PermissionsExt as _;

use super::{key, prepare_egress, KEY_FILE};
use crate::env::{Env, EGRESS_KEY};

#[test]
fn the_key_is_made_once_owner_only_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let first = key(dir.path()).unwrap();
    assert!(first.len() >= 40, "{first}");
    let mode = std::fs::metadata(dir.path().join(KEY_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(key(dir.path()).unwrap(), first);
}

#[test]
fn a_proxy_elsewhere_needs_the_key_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_str().unwrap();
    let env = |extra: &[(&str, &str)]| {
        let base = [
            ("MOOR_STATE_DIR", state),
            ("MOOR_EGRESS_URL", "wss://egress.test/egress"),
        ];
        Env::from_pairs(base.iter().chain(extra).copied())
    };
    let err = prepare_egress(env(&[])).err().unwrap();
    assert!(err.contains(EGRESS_KEY), "{err}");
    let kept = prepare_egress(env(&[(EGRESS_KEY, "given")])).unwrap();
    assert_eq!(kept.get(EGRESS_KEY), Some("given"));
    assert!(!dir.path().join(KEY_FILE).exists(), "no key of its own");
}
