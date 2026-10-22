//! Origin parsing tests.

use super::parse_origin;

#[test]
fn parse_origin_table() {
    let rows: &[(&str, Option<(&str, &str)>)] = &[
        ("https://github.com/o/n", Some(("o", "n"))),
        ("https://github.com/o/n.git", Some(("o", "n"))),
        ("https://github.com/o/n/", Some(("o", "n"))),
        ("https://GitHub.com/o/n", Some(("o", "n"))),
        ("https://user@github.com/o/n.git", Some(("o", "n"))),
        (
            "https://x-access-token:tok@github.com/o/n",
            Some(("o", "n")),
        ),
        ("ssh://git@github.com/o/n", Some(("o", "n"))),
        ("ssh://git@github.com/o/n.git", Some(("o", "n"))),
        ("git@github.com:o/n", Some(("o", "n"))),
        ("git@github.com:o/n.git", Some(("o", "n"))),
        ("https://gitlab.com/o/n", None),
        ("git@gitlab.com:o/n.git", None),
        ("https://github.example.com/o/n", None),
        ("https://ghe.corp/o/n", None),
        ("https://github.com.evil.com/o/n", None),
        ("https://evil.com/github.com/o/n", None),
        ("https://github.com/o/n/tree/main", None),
        ("git@github.com:o/n/extra", None),
        ("https://github.com/o", None),
        ("https://github.com/o/", None),
        ("https://github.com/", None),
        ("https://github.com", None),
        ("https://github.com//n", None),
        ("https://github.com/o/", None),
        ("https://github.com/o/.git", None),
        ("git@github.com:o/.git", None),
        ("git@github.com:/n", None),
        ("https://github.com/../n", None),
        ("https://github.com/o/..", None),
        ("https://github.com/o /n", None),
        ("https://github.com/o/n x", None),
        // Ports are refused, for every scheme.
        ("https://github.com:443/o/n", None),
        ("ssh://git@github.com:22/o/n", None),
        // Plain http and other schemes are refused.
        ("http://github.com/o/n", None),
        ("git://github.com/o/n", None),
        ("garbage", None),
        ("://", None),
        ("", None),
    ];
    for (input, want) in rows {
        let got = parse_origin(input).map(|r| (r.owner, r.name));
        let want = want.map(|(o, n)| (o.to_owned(), n.to_owned()));
        assert_eq!(got, want, "origin {input:?}");
    }
}

#[test]
fn parse_caps_the_owner_at_39_and_the_name_at_100_bytes() {
    use super::ForgeRepo;
    let at = |owner: usize, name: usize| {
        ForgeRepo::parse(&format!("{}/{}", "o".repeat(owner), "n".repeat(name)))
    };
    assert!(at(39, 100).is_ok());
    assert!(at(40, 1).is_err());
    assert!(at(1, 101).is_err());
}

#[test]
fn a_refused_repo_is_never_echoed() {
    let shown = super::ForgeRepo::parse("https://u:tok3n@gitlab.com/o/n")
        .unwrap_err()
        .to_string();
    assert!(!shown.contains("tok3n"), "{shown}");
}
