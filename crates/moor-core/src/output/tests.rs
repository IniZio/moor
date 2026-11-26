use super::{Output, OutputKind};

#[test]
fn a_kind_is_one_to_three_lowercase_words() {
    for good in [
        "github.comment",
        "figma.comment",
        "artifact.upload",
        "a.b-c.d1",
    ] {
        assert_eq!(OutputKind::parse(good).unwrap().as_str(), good);
    }
    for bad in [
        "",
        "GitHub.comment",
        "a..b",
        "a.b.c.d",
        "a b",
        ".a",
        &"x".repeat(65),
    ] {
        assert!(OutputKind::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn an_output_needs_an_https_url_and_a_one_line_title() {
    let ok = Output::parse(
        "github.comment",
        "https://example.com/acme/web/pull/5#c1",
        " Evidence ",
    )
    .unwrap();
    assert_eq!(ok.title(), "Evidence");
    assert_eq!(ok.url(), "https://example.com/acme/web/pull/5#c1");
    for url in [
        "http://example.com/x",
        "https://",
        "https:///x",
        "https://a b",
        "ftp://x",
    ] {
        assert!(Output::parse("github.comment", url, "t").is_err(), "{url}");
    }
    for title in ["", "  ", "a\nb", &"t".repeat(201)] {
        assert!(
            Output::parse("github.comment", "https://example.com", title).is_err(),
            "{title:?}"
        );
    }
}
