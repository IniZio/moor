use super::*;

fn get(addr: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer a.b.c\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    (
        status,
        out.split("\r\n\r\n").nth(1).unwrap_or("").to_string(),
    )
}

#[test]
fn exact_wins_over_prefix_and_last_reply_repeats() {
    let g = FakeGithub::start();
    g.route("GET", "/repos/*", vec![Reply::message(403, "no")]);
    g.route(
        "GET",
        "/repos/a/b",
        vec![Reply::ok(&json!({"n": 1})), Reply::ok(&json!({"n": 2}))],
    );
    let addr = g.url().replace("http://", "");
    assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":1}"#);
    assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":2}"#);
    assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":2}"#);
    assert_eq!(get(&addr, "/repos/other/c").0, 403);
    assert_eq!(get(&addr, "/nope").0, 404);
    assert_eq!(g.requests().len(), 5);
    assert_eq!(g.requests()[0].bearer(), Some("a.b.c"));
}

#[test]
fn base64_forms_decode() {
    assert_eq!(base64url_decode("aGk").unwrap(), b"hi");
    assert_eq!(base64url_decode("aGk=").unwrap(), b"hi");
    assert_eq!(base64url_decode("_-8").unwrap(), [0xff, 0xef]);
    assert!(base64url_decode("a b").is_none());
}
