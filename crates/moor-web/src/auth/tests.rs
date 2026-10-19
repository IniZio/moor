//! Tests for token authentication, CSRF checks and the token file.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

const TOKEN: &str = "s3cret-token";

/// Credentials for `GET /` with nothing set.
const fn bare<'a>() -> Credentials<'a> {
    Credentials {
        method: "GET",
        path: "/",
        bearer: None,
        cookie: None,
        query_token: None,
    }
}

#[test]
fn header_wins_over_cookie_and_query() {
    let creds = Credentials {
        bearer: Some(TOKEN),
        cookie: Some(TOKEN),
        query_token: Some(TOKEN),
        ..bare()
    };
    assert_eq!(authenticate(&creds, TOKEN), Some(Via::Header));
}

#[test]
fn cookie_is_accepted() {
    let creds = Credentials {
        bearer: Some("wrong"),
        cookie: Some(TOKEN),
        method: "POST",
        path: "/close",
        ..bare()
    };
    assert_eq!(authenticate(&creds, TOKEN), Some(Via::Cookie));
}

#[test]
fn wrong_token_is_rejected() {
    let creds = Credentials {
        bearer: Some("nope"),
        cookie: Some("nope"),
        query_token: Some("nope"),
        ..bare()
    };
    assert_eq!(authenticate(&creds, TOKEN), None);
    let empty = Credentials {
        bearer: Some(""),
        ..bare()
    };
    assert_eq!(authenticate(&empty, ""), None);
}

#[test]
fn query_is_accepted_only_on_get_root() {
    let query = Credentials {
        query_token: Some(TOKEN),
        ..bare()
    };
    assert_eq!(authenticate(&query, TOKEN), Some(Via::Query));
    for (method, path) in [("GET", "/state"), ("POST", "/"), ("POST", "/close")] {
        let creds = Credentials {
            method,
            path,
            ..query
        };
        assert_eq!(authenticate(&creds, TOKEN), None, "{method} {path}");
    }
}

#[test]
fn missing_credentials_are_rejected() {
    assert_eq!(authenticate(&bare(), TOKEN), None);
}

#[test]
fn cookie_header_is_http_only_and_strict() {
    assert_eq!(
        cookie_header("abc"),
        "moor_token=abc; HttpOnly; SameSite=Strict; Path=/"
    );
}

/// Headers that pass every check for `host`.
const fn good(host: &str) -> CsrfHeaders<'_> {
    CsrfHeaders {
        csrf: Some("1"),
        content_type: Some("application/json"),
        host: Some(host),
        origin: None,
        sec_fetch_site: None,
    }
}

#[test]
fn csrf_requirements_are_enforced() {
    let port = 4321;
    let loopback = format!("127.0.0.1:{port}");
    let localhost_origin = format!("http://localhost:{port}");
    let rejected = [
        CsrfHeaders {
            csrf: None,
            ..good(&loopback)
        },
        CsrfHeaders {
            origin: Some("http://evil.example"),
            ..good(&loopback)
        },
        CsrfHeaders {
            origin: Some("null"),
            ..good(&loopback)
        },
        CsrfHeaders {
            sec_fetch_site: Some("cross-site"),
            ..good(&loopback)
        },
        CsrfHeaders {
            content_type: Some("text/plain"),
            ..good(&loopback)
        },
        CsrfHeaders {
            content_type: None,
            ..good(&loopback)
        },
        // Origin must match Host exactly, even between loopback names.
        CsrfHeaders {
            origin: Some(&localhost_origin),
            ..good(&loopback)
        },
        // A rebound name is rejected even when Origin matches it.
        CsrfHeaders {
            origin: Some("http://evil.example:80"),
            ..good("evil.example:80")
        },
        good("127.0.0.1.evil.example"),
        CsrfHeaders {
            origin: Some("null"),
            ..good("localhost:9999")
        },
        CsrfHeaders {
            host: None,
            ..good("")
        },
    ];
    for headers in rejected {
        assert!(check_csrf(&headers).is_err(), "{headers:?}");
    }
}

#[test]
fn csrf_errors_carry_the_reason() {
    let h = good("127.0.0.1");
    let err = |h: CsrfHeaders<'_>| check_csrf(&h).unwrap_err();
    assert_eq!(
        err(CsrfHeaders { csrf: None, ..h }),
        "missing X-Moor-Csrf header"
    );
    assert_eq!(
        err(CsrfHeaders {
            content_type: Some("text/plain"),
            ..h
        }),
        "content type must be application/json"
    );
    assert_eq!(err(CsrfHeaders { host: None, ..h }), "missing Host header");
    assert_eq!(err(good("evil.example")), "bad host");
    assert_eq!(
        err(CsrfHeaders {
            origin: Some("null"),
            ..h
        }),
        "bad origin"
    );
    assert_eq!(
        err(CsrfHeaders {
            sec_fetch_site: Some("cross-site"),
            ..h
        }),
        "cross-site request"
    );
}

/// The browser's port may differ from the bound one.
#[test]
fn loopback_hosts_with_matching_origin_are_accepted() {
    let port = 4321;
    let loopback = format!("127.0.0.1:{port}");
    let loopback_origin = format!("http://127.0.0.1:{port}");
    let accepted = [
        CsrfHeaders {
            origin: Some("http://localhost:9999"),
            ..good("localhost:9999")
        },
        CsrfHeaders {
            origin: Some("http://[::1]:9999"),
            ..good("[::1]:9999")
        },
        CsrfHeaders {
            origin: Some("http://127.0.0.1"),
            ..good("127.0.0.1")
        },
        good(&loopback),
        CsrfHeaders {
            origin: Some(&loopback_origin),
            ..good(&loopback)
        },
        CsrfHeaders {
            sec_fetch_site: Some("same-origin"),
            ..good(&loopback)
        },
        CsrfHeaders {
            sec_fetch_site: Some("none"),
            content_type: Some(" Application/JSON; charset=utf-8"),
            ..good(&loopback)
        },
    ];
    for headers in accepted {
        assert_eq!(check_csrf(&headers), Ok(()), "{headers:?}");
    }
}

#[test]
fn loopback_host_names() {
    for host in ["127.0.0.1", "localhost:80", "[::1]", "[::1]:9"] {
        assert!(is_loopback_host(host), "{host}");
    }
    for host in ["", "evil.example", "[::1]x", "127.0.0.1.evil.example"] {
        assert!(!is_loopback_host(host), "{host}");
    }
}

/// Writes `body` to `dir/token` with `mode`.
fn token_file(dir: &Path, body: &str, mode: u32) -> PathBuf {
    let path = dir.join("token");
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn owner_only_file_is_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), "s3cret\n", 0o600);
    assert_eq!(read_token(&path).unwrap(), "s3cret");
}

#[test]
fn group_readable_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), "s3cret\n", 0o644);
    let err = read_token(&path).unwrap_err();
    assert!(err.contains("0600") && err.contains("token"), "{err}");
    assert!(!err.contains("s3cret"));
}

#[test]
fn empty_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), " \n", 0o600);
    assert!(read_token(&path).unwrap_err().contains("empty"));
}

#[test]
fn missing_file_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = read_token(&dir.path().join("nope")).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}
