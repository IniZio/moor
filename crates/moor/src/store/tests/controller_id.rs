#[test]
fn the_controller_id_is_made_once_per_store() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = super::open(a.path()).0.controller_id().unwrap();
    let (again, _) = super::open(a.path());
    assert_eq!(again.controller_id().unwrap(), first, "kept across opens");
    assert_eq!(again.controller_id().unwrap(), first, "kept across calls");
    let other = super::open(b.path()).0.controller_id().unwrap();
    assert_ne!(other, first, "a fresh store has its own");
    assert_eq!(first.as_str().len(), 8);
}
