//! Tests for user config parsing and validation.

use std::num::NonZeroU32;
use std::path::PathBuf;

use super::*;

fn err(doc: &str) -> ConfigError {
    parse_user("u.yaml", doc).expect_err(doc)
}

fn ok(doc: &str) -> UserConfig {
    let r = parse_user("u.yaml", doc);
    assert!(r.is_ok(), "{doc}: {r:?}");
    r.unwrap_or_default()
}

const CRED: &str = "credentials: {T: {env: X}}";

#[test]
fn empty_doc_is_defaults() {
    for doc in ["", "{}\n", "null"] {
        assert_eq!(parse_user("f", doc), Ok(UserConfig::default()), "{doc:?}");
    }
}

#[test]
fn defaults() {
    let c = UserConfig::default();
    assert_eq!(c.herdr_session(), "moor");
    assert_eq!(c.provider(), None);
    assert_eq!(c.github.identity, Identity::Auto);
    assert_eq!(BoxSize::default(), BoxSize::Small);
    assert_eq!(c.box_.size, None);
}

#[test]
fn provider_session_and_tools_parse() {
    let c = ok("box: {provider: static, size: large, max_boxes: 3, enforce_egress: true, sleep_after: 30m}\n\
         surface: {herdr: {session: work}}\n\
         tools: {crabbox: /opt/crabbox, git: git2}\n");
    assert_eq!(c.provider(), Some("static"));
    assert_eq!(c.box_.size, Some(BoxSize::Large));
    assert_eq!(c.box_.max_boxes, NonZeroU32::new(3));
    assert_eq!(c.box_.enforce_egress, Some(true));
    assert_eq!(c.box_.sleep_after.as_deref(), Some("30m"));
    assert_eq!(c.herdr_session(), "work");
    assert_eq!(c.tools.crabbox, Some(PathBuf::from("/opt/crabbox")));
    assert_eq!(c.tools.git, Some(PathBuf::from("git2")));
    assert_eq!(c.tools.herdr, None);
}

#[test]
fn accepted_docs() {
    let docs = [
        "box: {size: small}",
        "box: {size: xlarge, provider: custom-provider}",
        "box: {enforce_egress: true}",
        "github: {identity: you}",
        "github: {identity: bot}",
        "credentials: {A-b_1: {file: /f}}",
        "credentials: {A: {env: X}}",
        "credentials: {A: {cmd: [gh, auth, token]}}",
        "drivers: {crabbox: {max_boxes: 2}}",
        "dotfiles: {path: /abs, install: install.sh}",
        "dotfiles: {path: ~/dots}",
        "dotfiles: {path: /abs, install: ./i.sh}",
        "git: {name: N, email: e@example.com}",
        "agent: {model: haiku}",
        "egress: {allow: [Example.COM, '*.example.org']}",
        "box_credentials: [A, B]",
        "state_dir: /var/moor",
        "credentials: {T: {env: X}}\nmcp_servers: {m: {command: '${NOPE}', args: ['${T}'], env: {K: '${T}'}}, u: {url: 'http://h/${T}', headers: {X: '${T}'}}}",
    ];
    for doc in docs {
        ok(doc);
    }
    assert_eq!(
        ok("github: {identity: you}").github.identity.as_str(),
        "you"
    );
    assert_eq!(
        ok("box: {size: medium}").box_.size.map(BoxSize::as_str),
        Some("medium")
    );
}

#[test]
fn rejected_docs_with_paths() {
    let cases: &[(&str, &str)] = &[
        ("hostz: 1", "hostz"),
        ("box: {nope: 1}", "box.nope"),
        ("box: {providr: ssh}", "box.providr"),
        ("github: {nope: 1}", "github.nope"),
        ("reports: {backend: local}", "reports"),
        ("reports: {}", "reports"),
        ("drivers: {x: {ssh_port: 22}}", "drivers.x.ssh_port"),
        (
            "drivers: {x: {sleep_after_minutes: 5}}",
            "drivers.x.sleep_after_minutes",
        ),
        ("git: {nope: 1}", "git.nope"),
        ("dotfiles: {nope: 1}", "dotfiles.nope"),
        ("drivers: {crabbox: {nope: 1}}", "drivers.crabbox.nope"),
        ("credentials: {A: {env: X, nope: 1}}", "credentials.A.nope"),
        (
            "mcp_servers: {m: {command: x, nope: 1}}",
            "mcp_servers.m.nope",
        ),
    ];
    check_rejects(cases);
}

#[test]
fn rejected_values_with_paths() {
    let cases: &[(&str, &str)] = &[
        (
            "credentials: {GH_TOKEN: {file: /f}}",
            "credentials.GH_TOKEN",
        ),
        (
            "credentials: {bad name: {file: /f}}",
            "credentials.bad name",
        ),
        ("credentials: {A: {}}", "credentials.A"),
        ("credentials: {A: {file: /f, env: X}}", "credentials.A"),
        ("credentials: {A: {cmd: ['   ']}}", "credentials.A"),
        ("credentials: {A: {cmd: []}}", "credentials.A"),
        ("credentials: {A: {env: ''}}", "credentials.A"),
        ("github: {identity: Bot}", "github.identity"),
        ("github: {identity: other}", "github.identity"),
        ("box: {size: huge}", "box.size"),
        ("box: {enforce_egress: 'yes'}", "box.enforce_egress"),
        (
            "credentials: {A: {cmd: gh auth token}}",
            "credentials.A.cmd",
        ),
        ("dotfiles: {}", "dotfiles"),
        ("dotfiles: {path: relative/dir}", "dotfiles.path"),
        ("dotfiles: {path: /abs, install: ../x}", "dotfiles.install"),
        ("dotfiles: {path: /abs, install: /abs}", "dotfiles.install"),
        ("dotfiles: {dir: /abs}", "dotfiles.dir"),
        ("dotfiles: {path: /abs, git: x}", "dotfiles.git"),
        (
            "github: {app: {id: 1, key_path: ''}}",
            "github.app.key_path",
        ),
        ("agent: {location: box}", "agent.location"),
        ("egress: {allow: [a.com, 10.0.0.1]}", "egress.allow[1]"),
        ("box_credentials: [ANTHROPIC_API_KEY]", "box_credentials[0]"),
        ("box_credentials: [A, GH_TOKEN]", "box_credentials[1]"),
        ("box_credentials: [bad name]", "box_credentials[0]"),
        ("box_credentials: [A, A]", "box_credentials[1]"),
        ("box: [unclosed", ""),
        ("box: {max_boxes: many}", "box.max_boxes"),
    ];
    check_rejects(cases);
}

fn check_rejects(cases: &[(&str, &str)]) {
    for (doc, path) in cases {
        let e = err(doc);
        assert_eq!(e.path, *path, "{doc}: {e}");
        assert_eq!(e.file, "u.yaml", "{doc}");
    }
}

#[test]
fn mcp_server_rejects() {
    let cases = [
        ("m: {command: x, url: 'http://h'}", "mcp_servers.m"),
        ("m: {args: [a]}", "mcp_servers.m"),
        (
            "m: {command: x, args: ['${NOPE}']}",
            "mcp_servers.m.args[0]",
        ),
        (
            "m: {command: x, args: [a, '${NOPE}']}",
            "mcp_servers.m.args[1]",
        ),
        ("m: {url: 'http://h/${NOPE}'}", "mcp_servers.m.url"),
        (
            "m: {url: 'http://h', headers: {A: '${NOPE}'}}",
            "mcp_servers.m.headers.A",
        ),
        (
            "m: {command: x, env: {K: '${NOPE}'}}",
            "mcp_servers.m.env.K",
        ),
        ("bad name: {command: x}", "mcp_servers.bad name"),
        (
            "'${NOPE}': {command: '${NOPE}', args: ['${T}'], env: {'${NOPE}': '${T}'}}",
            "mcp_servers.${NOPE}",
        ),
    ];
    for (servers, path) in cases {
        let e = err(&format!("{CRED}\nmcp_servers: {{{servers}}}"));
        assert_eq!(e.path, path, "{servers}: {e}");
    }
}

#[test]
fn display_names_file_and_key_path() {
    let e = err("box: {providr: ssh}");
    assert!(e.to_string().starts_with("u.yaml:box.providr: "), "{e}");
    let e = err("agent: {location: box}");
    assert!(e.reason.contains("unknown field"), "{e}");
    assert!(e.to_string().contains("agent.location"), "{e}");
    let e = err("credentials: {GH_TOKEN: {env: X}}");
    assert_eq!(
        e.reason,
        check::not_gh_token("GH_TOKEN").unwrap_err(),
        "{e}"
    );
    assert!(err("box: [unclosed").reason.starts_with("invalid yaml"));
}

#[test]
fn credential_sources_parse() {
    let c = ok("credentials: {V: {cmd: [gh, token]}, F: {file: /f}}");
    assert_eq!(
        c.credentials.get("V").and_then(|c| c.cmd.clone()),
        Some(vec!["gh".to_owned(), "token".to_owned()])
    );
    assert_eq!(
        c.credentials.get("F").and_then(|c| c.file.clone()),
        Some(PathBuf::from("/f"))
    );
}

#[test]
fn string_cmd_is_a_type_error() {
    let e = err("credentials: {A: {cmd: gh auth token}}");
    assert_eq!(e.path, "credentials.A.cmd", "{e}");
    assert!(e.reason.contains("invalid type"), "{e}");
}

#[test]
fn zero_counts_are_rejected_by_type() {
    for (doc, path) in [
        ("box: {max_boxes: 0}", "box.max_boxes"),
        ("drivers: {c: {max_boxes: 0}}", "drivers.c.max_boxes"),
    ] {
        let e = err(doc);
        assert_eq!(e.path, path, "{doc}: {e}");
        assert!(e.reason.contains("nonzero"), "{doc}: {e}");
    }
}

#[test]
fn github_app_parses_and_is_strict() {
    let c = ok("github: {app: {id: 42, key_path: ~/app.pem, install: {org: acme}}}");
    let app = c.github.app.unwrap_or_else(|| unreachable!("app parsed"));
    assert_eq!(app.id, 42);
    assert_eq!(app.key_path, PathBuf::from("~/app.pem"));
    assert_eq!(app.install.len(), 1);
    let e = err("github: {app: {id: 1, key_path: /k, nope: 1}}");
    assert_eq!(e.path, "github.app.nope", "{e}");
    assert_eq!(err("github: {app: {id: 1}}").path, "github.app");
}

#[test]
fn mcp_refs() {
    let s = McpServer {
        args: vec!["a${X}b${Y}${X}".into(), "${open".into()],
        ..McpServer::default()
    };
    assert_eq!(s.refs(), ["X", "Y"]);
    let c = ok("credentials: {T: {env: X}, U: {env: Y}}\nmcp_servers: {m: {url: 'h/${U}', headers: {A: '${T}', B: '${U}'}}}");
    assert_eq!(
        c.mcp_servers
            .get("m")
            .map(McpServer::refs)
            .unwrap_or_default(),
        ["T", "U"]
    );
}

#[test]
fn credential_pass_env_parses_and_validates() {
    let c = ok("credentials: {A: {cmd: [op, read], pass_env: [OP_SESSION, HOME]}}");
    assert_eq!(
        c.credentials.get("A").map(|a| a.pass_env.clone()),
        Some(vec!["OP_SESSION".to_owned(), "HOME".to_owned()])
    );
    let e = err("credentials: {A: {cmd: [x], pass_env: [OK, 'bad-name']}}");
    assert_eq!(e.path, "credentials.A.pass_env[1]");
    let e = err("credentials: {A: {cmd: [x], pass_env: [X, X]}}");
    assert_eq!(e.path, "credentials.A.pass_env[1]");
    assert!(e.reason.contains("twice"), "{e}");
    let e = err("credentials: {A: {file: /f, pass_env: [X]}}");
    assert_eq!(e.path, "credentials.A.pass_env");
    let e = err("credentials: {A: {env: X, pass_env: [X]}}");
    assert_eq!(e.path, "credentials.A.pass_env");
    assert!(err("credentials: {A: {cmd: [x], pass_env: X}}")
        .to_string()
        .contains("pass_env"));
}

#[test]
fn reports_rejection_names_the_key() {
    let e = err("reports: {backend: local}");
    assert_eq!(e.path, "reports");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn box_credentials_duplicate_matches_helper() {
    let e = err("box_credentials: [A, A]");
    let want = check::box_credentials(&["A".into(), "A".into()]).expect_err("duplicate");
    assert_eq!(
        (e.path.as_str(), e.reason.as_str()),
        ("box_credentials[1]", want.1.as_str())
    );
}
