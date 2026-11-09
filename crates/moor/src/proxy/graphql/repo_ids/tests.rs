use super::split_id;

#[test]
fn only_a_plain_decimal_id_splits() {
    assert_eq!(split_id("/repositories/101"), Some((101, "")));
    assert_eq!(
        split_id("/repositories/101/pulls/2"),
        Some((101, "/pulls/2"))
    );
    assert_eq!(split_id("/repositories/0"), Some((0, "")));
    for path in [
        "/repositories",
        "/repositories/",
        "/repositories/0101/pulls",
        "/repositories/101x",
        "/repositories/-1",
        "/repositories/+1",
        "/repositories/99999999999999999999999",
        "/repositoriesx/1",
    ] {
        assert_eq!(split_id(path), None, "{path}");
    }
}
