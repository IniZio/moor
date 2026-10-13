use super::*;

fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn len_of(pairs: &[(&str, &str)]) -> Result<BodyLen, Ambiguous> {
    body_len(&fields(pairs))
}

#[test]
fn no_headers_means_no_body() {
    assert_eq!(len_of(&[("Host", "x")]), Ok(BodyLen::None));
    assert_eq!(len_of(&[]), Ok(BodyLen::None));
}

#[test]
fn single_content_length() {
    assert_eq!(len_of(&[("Content-Length", "5")]), Ok(BodyLen::Length(5)));
    assert_eq!(
        len_of(&[("content-length", " 0\t")]),
        Ok(BodyLen::Length(0))
    );
    assert_eq!(len_of(&[("CONTENT-LENGTH", "7")]), Ok(BodyLen::Length(7)));
}

#[test]
fn identical_list_collapses() {
    assert_eq!(
        len_of(&[("Content-Length", "5, 5")]),
        Ok(BodyLen::Length(5))
    );
}

#[test]
fn differing_list_is_ambiguous() {
    assert_eq!(len_of(&[("Content-Length", "5, 6")]), Err(Ambiguous));
}

#[test]
fn repeated_content_length_is_ambiguous() {
    let pairs = [("Content-Length", "5"), ("content-length", "5")];
    assert_eq!(len_of(&pairs), Err(Ambiguous));
}

#[test]
fn malformed_content_length_is_ambiguous() {
    for value in [
        "abc",
        "-1",
        "+5",
        "",
        "0x10",
        "1 2",
        "5,",
        "99999999999999999999999",
    ] {
        assert_eq!(
            len_of(&[("Content-Length", value)]),
            Err(Ambiguous),
            "value {value:?}"
        );
    }
}

#[test]
fn transfer_encoding_with_content_length_is_ambiguous() {
    let pairs = [("Transfer-Encoding", "chunked"), ("Content-Length", "5")];
    assert_eq!(len_of(&pairs), Err(Ambiguous));
}

#[test]
fn transfer_encoding_must_end_in_chunked() {
    assert_eq!(len_of(&[("Transfer-Encoding", "gzip")]), Err(Ambiguous));
    assert_eq!(
        len_of(&[("Transfer-Encoding", "chunked, gzip")]),
        Err(Ambiguous)
    );
    assert_eq!(
        len_of(&[("Transfer-Encoding", "gzip, chunked")]),
        Ok(BodyLen::Chunked)
    );
}

#[test]
fn transfer_encoding_ignores_case() {
    assert_eq!(
        len_of(&[("transfer-encoding", "CHUNKED")]),
        Ok(BodyLen::Chunked)
    );
}

#[test]
fn transfer_encoding_fields_concatenate() {
    let pairs = [
        ("Transfer-Encoding", "gzip"),
        ("Transfer-Encoding", "chunked"),
    ];
    assert_eq!(len_of(&pairs), Ok(BodyLen::Chunked));
    let reversed = [
        ("Transfer-Encoding", "chunked"),
        ("Transfer-Encoding", "gzip"),
    ];
    assert_eq!(len_of(&reversed), Err(Ambiguous));
}

#[test]
fn empty_transfer_coding_is_ambiguous() {
    for value in ["", "chunked,", ",chunked", "gzip,,chunked"] {
        assert_eq!(
            len_of(&[("Transfer-Encoding", value)]),
            Err(Ambiguous),
            "value {value:?}"
        );
    }
}

#[test]
fn caps_per_route() {
    assert_eq!(body_cap(&RouteId::new("forge.git")), 2 * 1024 * 1024 * 1024);
    assert_eq!(body_cap(&RouteId::new("forge.api")), 32 * 1024 * 1024);
    assert_eq!(body_cap(&RouteId::model()), 32 * 1024 * 1024);
}
