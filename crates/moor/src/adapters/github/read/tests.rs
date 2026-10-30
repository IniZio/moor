#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, GitAuthor, RepoAt};
use moor_core::{Secret, Sha};
use serde_json::{json, Value};

use crate::adapters::github::GitHub;

const TOKEN: &str = "ghp_Zq8xW3vK9mR2pL7nT4yB6cJ1dF5hG0sQ";
const DEFAULT_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TAGGED_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BRANCH_SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";

/// A loopback stand-in for api.github.com that answers one request.
struct FakeApi {
    url: String,
    seen: JoinHandle<Option<String>>,
}

impl FakeApi {
    /// Answers the request's variables with `answer`'s status and body.
    fn start(answer: impl FnOnce(&Value) -> (u16, String) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let seen = thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let request: Value = serde_json::from_str(&body).unwrap();
            let (status, reply) = answer(&request["variables"]);
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            Some(format!("{head}{body}"))
        });
        Self { url, seen }
    }

    /// The raw request the fake received.
    fn request(self) -> String {
        self.seen.join().unwrap().unwrap()
    }
}

fn token() -> Secret {
    Secret::new(TOKEN.as_bytes().to_vec())
}

fn read_via(api: &FakeApi, base: Option<&str>, branch: &str) -> Result<RepoAt, ForgeError> {
    GitHub::at(&api.url).read(&ForgeRepo::new("o", "n"), base, branch, &token())
}

fn assert_no_token_fragment(shown: &str) {
    for w in TOKEN.as_bytes().windows(4) {
        let w = std::str::from_utf8(w).unwrap();
        assert!(!shown.contains(w), "token fragment {w} leaked into {shown}");
    }
}

/// Answers for repo `o/n` with no config files, as the account `viewer`.
fn viewer_reply(viewer: Value, permission: &str) -> impl FnOnce(&Value) -> (u16, String) + Send {
    let permission = permission.to_owned();
    move |_| {
        let at = commit(DEFAULT_SHA, &[]);
        ok(&json!({"data": {
            "viewer": viewer,
            "repository": {"viewerPermission": permission, "branch": null, "defaultBranchRef": {"target": at}},
        }}))
    }
}

fn errors_reply(kinds: &[Value]) -> impl FnOnce(&Value) -> (u16, String) + Send {
    let errors: Vec<Value> = kinds.iter().map(|k| json!({"type": k})).collect();
    move |_| ok(&json!({"data": null, "errors": errors}))
}

fn blob(text: &str) -> Value {
    json!({"object": {"__typename": "Blob", "byteSize": text.len(), "isBinary": false, "text": text}})
}

fn commit(oid: &str, files: &[(&str, Value)]) -> Value {
    let mut c = json!({"__typename": "Commit", "oid": oid});
    for (field, entry) in files {
        c[*field] = entry.clone();
    }
    c
}

/// Answers like GitHub for a repo `o/n` whose default branch has all three files, whose
/// annotated tag `v1` points at a commit with only `.moor.yaml`, and whose branch `moor/t` exists.
fn github(vars: &Value, permission: &str) -> (u16, String) {
    let default = commit(
        DEFAULT_SHA,
        &[
            ("moorYaml", blob("setup: make\n")),
            ("mcpJson", blob("{\"mcpServers\":{}}")),
            ("claudeSettings", blob("{\"permissions\":{}}")),
        ],
    );
    let tagged = commit(
        TAGGED_SHA,
        &[
            ("moorYaml", blob("setup: old\n")),
            ("mcpJson", Value::Null),
            ("claudeSettings", Value::Null),
        ],
    );
    let branch = match vars["branch"].as_str() {
        Some("refs/heads/moor/t") => json!({"target": {"__typename": "Commit", "oid": BRANCH_SHA}}),
        _ => Value::Null,
    };
    let mut repo = json!({"viewerPermission": permission, "branch": branch});
    if vars["hasBase"] == json!(true) {
        repo["object"] = match vars["base"].as_str() {
            Some("v1") => json!({"__typename": "Tag", "oid": "dd".repeat(20), "target": tagged}),
            Some("main") => default,
            Some("refs/heads/moor/t") => commit(BRANCH_SHA, &[]),
            Some("moor/t") => {
                json!({"__typename": "Tag", "oid": "ee".repeat(20), "target": tagged})
            }
            _ => Value::Null,
        };
    } else {
        repo["defaultBranchRef"] = json!({"target": default});
    }
    let viewer = json!({"login": "octo", "databaseId": 42, "name": "Octo Cat"});
    ok(&json!({"data": {"viewer": viewer, "repository": repo}}))
}

fn ok(reply: &Value) -> (u16, String) {
    (200, reply.to_string())
}

fn sha(s: &str) -> Sha {
    Sha::parse(s).unwrap()
}

#[test]
fn default_branch_resolves_with_its_files() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    let at = read_via(&api, None, "moor/t").unwrap();
    assert_eq!(at.base, sha(DEFAULT_SHA));
    assert_eq!(at.files.moor_yaml.as_deref(), Some("setup: make\n"));
    assert_eq!(at.files.mcp_json.as_deref(), Some("{\"mcpServers\":{}}"));
    assert_eq!(
        at.files.claude_settings.as_deref(),
        Some("{\"permissions\":{}}")
    );
    assert_eq!(at.branch_tip, Some(sha(BRANCH_SHA)));
    assert!(at.can_push);
    assert_eq!(
        at.author,
        GitAuthor {
            name: "Octo Cat".to_owned(),
            email: "42+octo@users.noreply.github.com".to_owned(),
        }
    );
}

#[test]
fn given_base_resolves_through_its_tag_with_files_at_that_commit() {
    let api = FakeApi::start(|v| github(v, "READ"));
    let at = read_via(&api, Some("v1"), "moor/new").unwrap();
    assert_eq!(at.base, sha(TAGGED_SHA));
    assert_eq!(at.files.moor_yaml.as_deref(), Some("setup: old\n"));
    assert_eq!(at.branch_tip, None);
    assert!(!at.can_push);
}

#[test]
fn missing_files_are_none() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    let files = read_via(&api, Some("v1"), "moor/t").unwrap().files;
    assert_eq!(files.mcp_json, None);
    assert_eq!(files.claude_settings, None);
}

#[test]
fn base_names_the_requested_ref() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    assert_eq!(
        read_via(&api, Some("main"), "moor/t").unwrap().base,
        sha(DEFAULT_SHA)
    );
}

#[test]
fn a_base_naming_the_box_branch_reads_the_branch_not_a_tag_of_that_name() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    let at = read_via(&api, Some("moor/t"), "moor/t").unwrap();
    assert_eq!(at.base, sha(BRANCH_SHA));
    assert_eq!(at.branch_tip, Some(sha(BRANCH_SHA)));
}

#[test]
fn bad_ref_is_unknown_base() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    assert_eq!(
        read_via(&api, Some("no-such-ref"), "moor/t"),
        Err(ForgeError::UnknownBase("no-such-ref".to_owned()))
    );
}

#[test]
fn not_found_is_not_visible() {
    let api = FakeApi::start(|_| {
        ok(&json!({
            "data": {"viewer": {"login": "octo", "databaseId": 42, "name": null}, "repository": null},
            "errors": [{"type": "NOT_FOUND", "path": ["repository"], "message": "Could not resolve to a Repository"}],
        }))
    });
    assert_eq!(read_via(&api, None, "moor/t"), Err(ForgeError::NotVisible));
}

#[test]
fn http_401_is_rejected() {
    let api = FakeApi::start(|_| (401, json!({"message": "Bad credentials"}).to_string()));
    assert_eq!(read_via(&api, None, "moor/t"), Err(ForgeError::Rejected));
}

fn moor_yaml_reply(entry: Value) -> impl FnOnce(&Value) -> (u16, String) + Send {
    move |_| {
        let at = commit(DEFAULT_SHA, &[("moorYaml", entry)]);
        ok(&json!({"data": {
            "viewer": {"login": "octo", "databaseId": 42, "name": null},
            "repository": {"viewerPermission": "WRITE", "branch": null, "defaultBranchRef": {"target": at}},
        }}))
    }
}

#[test]
fn binary_moor_yaml_is_unreadable() {
    let api = FakeApi::start(moor_yaml_reply(json!({"object": {
        "__typename": "Blob", "byteSize": 10, "isBinary": true, "text": null,
    }})));
    assert_eq!(
        read_via(&api, None, "moor/t"),
        Err(ForgeError::Unreadable(".moor.yaml"))
    );
}

#[test]
fn oversized_moor_yaml_is_unreadable() {
    let big = "a".repeat(64 * 1024 + 1);
    let api = FakeApi::start(moor_yaml_reply(blob(&big)));
    assert_eq!(
        read_via(&api, None, "moor/t"),
        Err(ForgeError::Unreadable(".moor.yaml"))
    );
}

#[test]
fn moor_yaml_at_the_size_cap_is_read() {
    let max = "a".repeat(64 * 1024);
    let api = FakeApi::start(moor_yaml_reply(blob(&max)));
    let at = read_via(&api, None, "moor/t").unwrap();
    assert_eq!(at.files.moor_yaml.map(|t| t.len()), Some(64 * 1024));
    assert_eq!(
        at.author.name, "octo",
        "an account with no profile name commits as its login"
    );
}

type Answer = Box<dyn FnOnce(&Value) -> (u16, String) + Send>;

/// Each answer echoes the token back in its body, as a hostile or broken server might.
fn hostile_answers() -> Vec<Answer> {
    let echo = format!("Bearer {TOKEN}");
    vec![
        Box::new(|v| github(v, "WRITE")),
        Box::new({
            let echo = echo.clone();
            move |_| (401, json!({"message": echo}).to_string())
        }),
        Box::new({
            let echo = echo.clone();
            move |_| (502, echo)
        }),
        Box::new(move |_| {
            ok(&json!({
                "data": null,
                "errors": [{"type": format!("RATE_LIMITED {echo}"), "message": echo}],
            }))
        }),
        Box::new(|_| {
            let kind = format!("RATE_LIMITED {TOKEN} Bearer");
            ok(&json!({"data": null, "errors": [{"type": kind}]}))
        }),
        Box::new(|_| (200, format!("not json {TOKEN}"))),
        Box::new(|_| ok(&json!({"data": {"viewer": TOKEN, "repository": null}}))),
    ]
}

#[test]
fn token_is_sent_only_in_the_authorization_header_and_never_in_errors() {
    for answer in hostile_answers() {
        let api = FakeApi::start(answer);
        let shown = match read_via(&api, None, "moor/t") {
            Ok(at) => format!("{at:?}"),
            Err(e) => format!("{e} {e:?}"),
        };
        assert_no_token_fragment(&shown);
        let request = api.request();
        assert_eq!(request.matches(TOKEN).count(), 1, "{request}");
        let line = request.lines().find(|l| l.contains(TOKEN)).unwrap();
        assert_eq!(
            line.to_ascii_lowercase(),
            format!("authorization: bearer {}", TOKEN.to_ascii_lowercase())
        );
    }
}

#[test]
fn other_graphql_errors_are_unavailable() {
    let api = FakeApi::start(|_| {
        ok(&json!({"data": null, "errors": [{"type": "RATE_LIMITED", "message": "slow down"}]}))
    });
    assert_eq!(
        read_via(&api, None, "moor/t"),
        Err(ForgeError::Unavailable(
            "GitHub GraphQL errors: RATE_LIMITED".to_owned()
        ))
    );
}

fn graphql_errors(names: &str) -> Result<RepoAt, ForgeError> {
    Err(ForgeError::Unavailable(format!(
        "GitHub GraphQL errors: {names}"
    )))
}

#[test]
fn allow_listed_error_types_are_named() {
    for kind in [
        "FORBIDDEN",
        "RATE_LIMITED",
        "INSUFFICIENT_SCOPES",
        "SERVICE_UNAVAILABLE",
        "INTERNAL",
        "MAX_NODE_LIMIT_EXCEEDED",
    ] {
        let api = FakeApi::start(errors_reply(&[json!(kind)]));
        assert_eq!(read_via(&api, None, "moor/t"), graphql_errors(kind));
    }
}

#[test]
fn other_error_types_are_unknown_and_carry_no_server_bytes() {
    for kind in [
        json!(format!("RATE_LIMITED {TOKEN} Bearer")),
        json!("A".repeat(2 * 1024 * 1024)),
        json!("rate_limited"),
        json!("FORBIDDEN "),
        Value::Null,
    ] {
        let api = FakeApi::start(errors_reply(&[kind]));
        assert_eq!(read_via(&api, None, "moor/t"), graphql_errors("UNKNOWN"));
    }
}

#[test]
fn error_types_are_deduplicated_and_at_most_three() {
    let kinds = ["FORBIDDEN", "FORBIDDEN", "INTERNAL", "X", "RATE_LIMITED"].map(|k| json!(k));
    let api = FakeApi::start(errors_reply(&kinds));
    assert_eq!(
        read_via(&api, None, "moor/t"),
        graphql_errors("FORBIDDEN, INTERNAL, UNKNOWN")
    );
}

#[test]
fn not_found_mixed_with_another_error_is_unavailable() {
    let api = FakeApi::start(errors_reply(&[json!("NOT_FOUND"), json!("FORBIDDEN")]));
    assert_eq!(
        read_via(&api, None, "moor/t"),
        graphql_errors("NOT_FOUND, FORBIDDEN")
    );
    let api = FakeApi::start(|v| {
        let mut reply: Value = serde_json::from_str(&github(v, "WRITE").1).unwrap();
        reply["errors"] = json!([{"type": "NOT_FOUND"}, {"type": "INTERNAL"}]);
        ok(&reply)
    });
    assert_eq!(
        read_via(&api, None, "moor/t"),
        graphql_errors("NOT_FOUND, INTERNAL")
    );
}

#[test]
fn only_write_maintain_and_admin_can_push() {
    for (permission, can_push) in [
        ("ADMIN", true),
        ("MAINTAIN", true),
        ("WRITE", true),
        ("TRIAGE", false),
        ("READ", false),
    ] {
        let api = FakeApi::start(move |v| github(v, permission));
        let at = read_via(&api, None, "moor/t").unwrap();
        assert_eq!(at.can_push, can_push, "{permission}");
    }
}

#[test]
fn binary_blob_with_text_is_unreadable() {
    let api = FakeApi::start(moor_yaml_reply(json!({"object": {
        "__typename": "Blob", "byteSize": 3, "isBinary": true, "text": "abc",
    }})));
    assert_eq!(
        read_via(&api, None, "moor/t"),
        Err(ForgeError::Unreadable(".moor.yaml"))
    );
}

fn author_of(login: &str, name: &Value) -> Result<GitAuthor, ForgeError> {
    let viewer = json!({"login": login, "databaseId": 42, "name": name});
    let api = FakeApi::start(viewer_reply(viewer, "WRITE"));
    read_via(&api, None, "moor/t").map(|at| at.author)
}

#[test]
fn login_must_be_shaped_like_a_github_login() {
    let unexpected = Err(ForgeError::Unavailable(
        "GitHub sent a reply of an unexpected shape".to_owned(),
    ));
    for login in [
        String::new(),
        "a".repeat(40),
        "a.b".to_owned(),
        "a_b".to_owned(),
        "a b".to_owned(),
        "\u{e9}".to_owned(),
        "a@evil".to_owned(),
    ] {
        assert_eq!(author_of(&login, &Value::Null), unexpected, "{login:?}");
    }
    for login in ["a".repeat(39), "Octo-Cat-1".to_owned()] {
        let author = author_of(&login, &Value::Null).unwrap();
        assert_eq!(author.name, login);
        assert_eq!(author.email, format!("42+{login}@users.noreply.github.com"));
    }
}

#[test]
fn unusable_profile_name_falls_back_to_the_login() {
    for name in [
        "   ".to_owned(),
        "\u{3000}".to_owned(),
        "x".repeat(256),
        "\u{e9}".repeat(128),
        "Octo\nCat".to_owned(),
        "Octo\u{7f}".to_owned(),
        "Octo\u{85}Cat".to_owned(),
        "\u{1b}[31mOcto".to_owned(),
        "Octo\u{202e}taC".to_owned(),
        "Octo\u{200b}".to_owned(),
        "\u{feff}Octo".to_owned(),
        "Octo\u{2028}Cat".to_owned(),
        "Octo\u{2029}Cat".to_owned(),
        "Octo\u{2066}".to_owned(),
        "Octo\u{ad}".to_owned(),
        "Octo\u{e0041}".to_owned(),
    ] {
        assert_eq!(
            author_of("octo", &json!(name)).unwrap().name,
            "octo",
            "{name:?}"
        );
    }
    let longest = "\u{e9}".repeat(127) + "x";
    assert_eq!(author_of("octo", &json!(longest)).unwrap().name, longest);
}

mod transport;

fn file_reply(text: String) -> impl FnOnce(&Value) -> (u16, String) + Send {
    move |_| {
        let target = commit(DEFAULT_SHA, &[("file", blob(&text))]);
        ok(&json!({"data": {"repository": {"defaultBranchRef": {"target": target}}}}))
    }
}

fn file_via(api: &FakeApi) -> Result<(Sha, Option<String>), ForgeError> {
    GitHub::at(&api.url).read_file_at(
        &ForgeRepo::new("o", "n"),
        None,
        ".claude-plugin/marketplace.json",
        &token(),
    )
}

#[test]
fn a_profile_file_past_the_config_cap_is_read() {
    let big = "a".repeat(200 * 1024);
    let api = FakeApi::start(file_reply(big.clone()));
    assert_eq!(file_via(&api), Ok((sha(DEFAULT_SHA), Some(big))));
}

#[test]
fn a_profile_file_past_a_mebibyte_is_unreadable() {
    let api = FakeApi::start(file_reply("a".repeat(1024 * 1024 + 1)));
    assert_eq!(
        file_via(&api),
        Err(ForgeError::Unreadable(".claude-plugin/marketplace.json"))
    );
}
