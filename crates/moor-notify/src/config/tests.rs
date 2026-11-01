use std::path::Path;

use super::{parse, DEFAULT_EVENTS};

/// A secret reader that hands back the file name as the key.
#[expect(clippy::unnecessary_wraps, reason = "it stands in for the real reader")]
fn named(path: &Path) -> Result<Vec<u8>, String> {
    Ok(path.display().to_string().into_bytes())
}

/// Parses `text` with [`named`] secrets.
fn hooks(text: &str) -> Result<Vec<super::Hook>, String> {
    parse("n.yaml", text, named)
}

#[test]
fn a_hook_without_events_gets_the_defaults() {
    let got = hooks(
        "hooks:\n  - email: A@Example.com\n    url: https://hooks.example.com/moor\n    secret_file: /s\n",
    )
    .unwrap();
    assert_eq!(got.len(), 1, "one hook: {got:?}");
    let hook = got.first().unwrap();
    assert_eq!(hook.email.as_str(), "a@example.com");
    assert_eq!(hook.url, "https://hooks.example.com/moor");
    assert_eq!(hook.secret, b"/s");
    assert_eq!(hook.events, DEFAULT_EVENTS);
}

#[test]
fn named_events_are_kept_in_order_without_repeats() {
    let got = hooks(
        "hooks:\n  - email: a@example.com\n    url: https://h.example.com/\n    secret_file: /s\n    events: [relaunched, idled, relaunched]\n",
    )
    .unwrap();
    assert_eq!(got.first().unwrap().events, ["relaunched", "idled"]);
}

#[test]
fn loopback_http_is_allowed() {
    for url in [
        "http://127.0.0.1:9000/hook",
        "http://localhost/x",
        "http://[::1]:80/",
    ] {
        let text =
            format!("hooks:\n  - email: a@example.com\n    url: \"{url}\"\n    secret_file: /s\n");
        assert!(hooks(&text).is_ok(), "{url} should be allowed");
    }
}

#[test]
fn refuses_what_it_cannot_use() {
    let cases = [
        ("", "list at least one hook"),
        ("hooks: []\n", "list at least one hook"),
        ("hookz: []\n", "hookz"),
        (
            "hooks:\n  - email: nope\n    url: https://h.example.com/\n    secret_file: /s\n",
            "hooks[0]: email `nope`",
        ),
        (
            "hooks:\n  - email: a@example.com\n    url: http://h.example.com/\n    secret_file: /s\n",
            "must be https://",
        ),
        (
            "hooks:\n  - email: a@example.com\n    url: ftp://127.0.0.1/\n    secret_file: /s\n",
            "must be https://",
        ),
        (
            "hooks:\n  - email: a@example.com\n    url: https://h.example.com/\n    secret_file: /s\n    events: []\n",
            "list at least one kind",
        ),
        (
            "hooks:\n  - email: a@example.com\n    url: https://h.example.com/\n    secret_file: /s\n    events: [Idled]\n",
            "`Idled` is not an event kind",
        ),
        (
            "hooks:\n  - email: a@example.com\n    url: https://h.example.com/\n    secret_file: /s\n    box: x\n",
            "box",
        ),
    ];
    for (text, want) in cases {
        let err = hooks(text).unwrap_err();
        assert!(err.contains(want), "{text:?}: {err} should say {want}");
    }
}

#[test]
fn a_secret_file_must_be_owner_only_and_non_empty() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k");
    std::fs::write(&path, "key\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(super::read_secret(&path).unwrap_err().contains("mode 0600"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(super::read_secret(&path).unwrap(), b"key");
    std::fs::write(&path, "\n").unwrap();
    assert!(super::read_secret(&path).unwrap_err().contains("is empty"));
}
