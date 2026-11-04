#[test]
fn the_cache_salt_is_made_once_per_state_dir() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = crate::store::tests::open(a.path()).0.cache_salt().unwrap();
    let (again, _) = crate::store::tests::open(a.path());
    assert_eq!(again.cache_salt().unwrap(), first, "kept across opens");
    assert_eq!(again.cache_salt().unwrap(), first, "kept across calls");
    let other = crate::store::tests::open(b.path()).0.cache_salt().unwrap();
    assert_ne!(other, first, "a fresh state dir has its own");
}
