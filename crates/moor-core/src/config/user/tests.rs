//! Tests for user config parsing and validation.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

mod mcp;
mod service;

fn err(doc: &str) -> ConfigError {
    parse_user("u.yaml", doc).expect_err(doc)
}

fn ok(doc: &str) -> UserConfig {
    let r = parse_user("u.yaml", doc);
    assert!(r.is_ok(), "{doc}: {r:?}");
    r.unwrap_or_default()
}

#[test]
fn empty_doc_is_defaults() {
    for doc in ["", "{}\n", "null"] {
        assert_eq!(parse_user("f", doc), Ok(UserConfig::default()), "{doc:?}");
    }
}

#[test]
fn valueless_sections_are_defaults() {
    for key in [
        "box",
        "surface",
        "tools",
        "drivers",
        "agent",
        "box_credentials",
        "state_dir",
    ] {
        for doc in [format!("{key}:"), format!("{key}: ~")] {
            let r = parse_user("u.yaml", &doc);
            assert_eq!(r, Ok(UserConfig::default()), "{doc:?}");
        }
    }
}

#[test]
fn leading_zero_text_needs_quotes() {
    let c = ok("agent: {model: \"0755\"}\nstate_dir: '0755'");
    assert_eq!(c.agent.model.as_deref(), Some("0755"));
    assert_eq!(c.state_dir, Some(PathBuf::from("0755")));
    for (doc, path) in [
        ("agent: {model: 0755}", "agent.model"),
        ("state_dir: 0755", "state_dir"),
    ] {
        let e = err(doc);
        assert_eq!(e.path, path, "{e}");
        assert!(e.reason.contains("must be quoted"), "{e}");
    }
}

#[test]
fn defaults() {
    let c = UserConfig::default();
    assert_eq!(c.herdr_session(), "moor");
    assert_eq!(c.provider(), None);
    assert_eq!(BoxSize::default(), BoxSize::Small);
    assert_eq!(c.box_.size, None);
}

#[test]
fn sleep_after_defaults_and_parses() {
    let c = UserConfig::default();
    assert_eq!(c.box_.sleep_after(), DEFAULT_SLEEP_AFTER);
    assert_eq!(c.box_.sleep_after(), Duration::from_mins(30));
    assert_eq!(c.box_.sleep_after_label(), "30m");
    let c = ok("box: {sleep_after: 45m}");
    assert_eq!(c.box_.sleep_after(), Duration::from_mins(45));
    assert_eq!(c.box_.sleep_after_label(), "45m");
    let c = ok("box: {sleep_after: 2h}");
    assert_eq!(c.box_.sleep_after(), Duration::from_hours(2));
    assert_eq!(c.box_.sleep_after_label(), "2h");
}

#[test]
fn provider_session_and_tools_parse() {
    let c = ok(
        "box: {provider: static, size: large, max_boxes: 3, sleep_after: 30m}\n\
         surface: {herdr: {session: work}}\n\
         tools: {crabbox: /opt/crabbox, git: git2}\n",
    );
    assert_eq!(c.provider(), Some("static"));
    assert_eq!(c.box_.size, Some(BoxSize::Large));
    assert_eq!(c.box_.max_boxes, NonZeroU32::new(3));
    assert_eq!(c.box_.sleep_after.as_deref(), Some("30m"));
    assert_eq!(c.herdr_session(), "work");
    assert_eq!(c.surface.kind, SurfaceKind::Herdr);
    assert_eq!(c.tools.crabbox, Some(PathBuf::from("/opt/crabbox")));
    assert_eq!(c.tools.git, Some(PathBuf::from("git2")));
    assert_eq!(c.tools.herdr, None);
}

#[test]
fn accepted_docs() {
    let docs = [
        "box: {size: small}",
        "box: {size: xlarge, provider: custom-provider}",
        "drivers: {crabbox: {max_boxes: 2}}",
        "drivers: {crabbox: {lookup_timeout_secs: 30}}",
        "dotfiles: {path: /abs, install: install.sh}",
        "dotfiles: {path: ~/dots}",
        "dotfiles: {path: /abs, install: ./i.sh}",
        "agent: {model: haiku}",
        "box_credentials: [npm, pypi-token]",
        "state_dir: /var/moor",
        "mcp_servers: {m: {command: '${NOPE}', args: [a]}, u: {url: 'https://h.example/mcp', headers: {X: '${t}'}}}",
    ];
    for doc in docs {
        ok(doc);
    }
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
        ("reports: {backend: local}", "reports"),
        ("reports: {}", "reports"),
        ("drivers: {x: {ssh_port: 22}}", "drivers.x.ssh_port"),
        (
            "drivers: {x: {sleep_after_minutes: 5}}",
            "drivers.x.sleep_after_minutes",
        ),
        ("dotfiles: {nope: 1}", "dotfiles.nope"),
        ("drivers: {crabbox: {nope: 1}}", "drivers.crabbox.nope"),
        (
            "mcp_servers: {m: {command: x, nope: 1}}",
            "mcp_servers.m.nope",
        ),
    ];
    check_rejects(cases);
}

#[test]
fn git_section_is_unknown() {
    check_rejects(&[("git:\n  name: U\n", "git")]);
}

#[test]
fn rejected_values_with_paths() {
    let cases: &[(&str, &str)] = &[
        ("box: {size: huge}", "box.size"),
        ("dotfiles: {}", "dotfiles"),
        ("dotfiles: {path: relative/dir}", "dotfiles.path"),
        ("dotfiles: {path: /abs, install: ../x}", "dotfiles.install"),
        ("dotfiles: {path: /abs, install: /abs}", "dotfiles.install"),
        ("dotfiles: {dir: /abs}", "dotfiles.dir"),
        ("dotfiles: {path: /abs, git: x}", "dotfiles.git"),
        ("agent: {location: box}", "agent.location"),
        ("box_credentials: [ANTHROPIC_API_KEY]", "box_credentials[0]"),
        ("box_credentials: [a, GH_TOKEN]", "box_credentials[1]"),
        ("box_credentials: [bad name]", "box_credentials[0]"),
        ("box_credentials: [a, a]", "box_credentials[1]"),
        ("box: [unclosed", ""),
        ("box: {max_boxes: many}", "box.max_boxes"),
        ("box: {sleep_after: soon}", "box.sleep_after"),
        ("box: {sleep_after: 0m}", "box.sleep_after"),
        ("box: {sleep_after: '10'}", "box.sleep_after"),
        ("box: {sleep_after: 5x}", "box.sleep_after"),
        ("box: {sleep_after: ''}", "box.sleep_after"),
        (
            "box: {sleep_after: 99999999999999999999d}",
            "box.sleep_after",
        ),
        (
            "box: {sleep_after: 9999999999999999999d}",
            "box.sleep_after",
        ),
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
            "'${NOPE}': {command: '${NOPE}', args: ['${t}'], env: {'${NOPE}': '${t}'}}",
            "mcp_servers.${NOPE}",
        ),
    ];
    for (servers, path) in cases {
        let e = err(&format!("mcp_servers: {{{servers}}}"));
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
    assert!(err("box: [unclosed").reason.starts_with("invalid yaml"));
}

#[test]
fn zero_counts_are_rejected_by_type() {
    for (doc, path) in [
        ("box: {max_boxes: 0}", "box.max_boxes"),
        ("drivers: {c: {max_boxes: 0}}", "drivers.c.max_boxes"),
        (
            "drivers: {c: {lookup_timeout_secs: 0}}",
            "drivers.c.lookup_timeout_secs",
        ),
    ] {
        let e = err(doc);
        assert_eq!(e.path, path, "{doc}: {e}");
        assert!(e.reason.contains("nonzero"), "{doc}: {e}");
    }
}

#[test]
fn strip_env_parses() {
    let c = ok("drivers: {external: {strip_env: [PLATFORM_TOKEN]}}");
    let got = c.drivers.get("external").map(|d| d.strip_env.clone());
    assert_eq!(got, Some(vec!["PLATFORM_TOKEN".to_owned()]));
}

#[test]
fn strip_env_rejects_bad_names() {
    for doc in [
        r#"drivers: {external: {strip_env: ["1BAD"]}}"#,
        r#"drivers: {external: {strip_env: [OK, "A-B"]}}"#,
    ] {
        let e = err(doc);
        assert!(
            e.path.starts_with("drivers.external.strip_env["),
            "{doc}: {e}"
        );
    }
}

#[test]
fn lookup_timeout_secs_parses() {
    let c = ok("drivers: {crabbox: {lookup_timeout_secs: 30}}");
    let timeout = c.drivers.get("crabbox").and_then(|d| d.lookup_timeout_secs);
    assert_eq!(timeout, NonZeroU32::new(30));
}

#[test]
fn mcp_refs() {
    let s = McpServer {
        args: vec!["a${X}b${Y}${X}".into(), "${open".into()],
        ..McpServer::default()
    };
    assert_eq!(s.refs(), ["X", "Y"]);
    let c = ok(
        "mcp_servers: {m: {url: 'https://h.example/mcp', headers: {A: 'Bearer ${t}', B: plain}}}",
    );
    assert_eq!(
        c.mcp_servers
            .get("m")
            .map(McpServer::refs)
            .unwrap_or_default(),
        ["t"]
    );
}

#[test]
fn reports_rejection_names_the_key() {
    let e = err("reports: {backend: local}");
    assert_eq!(e.path, "reports");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn box_credentials_duplicate_matches_helper() {
    let e = err("box_credentials: [a, a]");
    let want = check::box_credentials(
        &["a".into(), "a".into()],
        &std::collections::BTreeMap::new(),
    )
    .expect_err("duplicate");
    assert_eq!(
        (e.path.as_str(), e.reason.as_str()),
        ("box_credentials[1]", want.1.as_str())
    );
}

#[test]
fn removed_keys_are_unknown() {
    for (doc, path) in [
        ("box: {enforce_egress: true}", "box.enforce_egress"),
        ("egress: {allow: [x.io]}", "egress"),
    ] {
        let e = err(doc);
        assert_eq!(e.path, path, "{doc}");
        assert!(e.reason.contains("unknown field"), "{e}");
    }
}

#[test]
fn controller_socket_parses() {
    let c = ok("controller: {socket: /tmp/x.sock}");
    assert_eq!(c.controller.socket, Some(PathBuf::from("/tmp/x.sock")));
}

#[test]
fn controller_section_is_optional() {
    assert_eq!(ok("{}").controller.socket, None);
    assert_eq!(ok("controller: {}").controller.socket, None);
}

#[test]
fn controller_unknown_key_is_named() {
    assert!(err("controller: {path: x}").to_string().contains("path"));
}

#[test]
fn controller_empty_socket_is_rejected() {
    assert_eq!(err("controller: {socket: ''}").path, "controller.socket");
}

#[test]
fn controller_ssh_parses() {
    let c = ok("controller: {ssh: newman@minion}");
    assert_eq!(c.controller.ssh.as_deref(), Some("newman@minion"));
    assert_eq!(ok("{}").controller.ssh, None);
}

#[test]
fn controller_bad_ssh_is_rejected() {
    for doc in [
        "controller: {ssh: ''}",
        "controller: {ssh: '-oProxyCommand=x'}",
        "controller: {ssh: 'a b'}",
        "controller: {ssh: \"a\\tb\"}",
        "controller: {ssh: \"a\\nb\"}",
    ] {
        assert_eq!(err(doc).path, "controller.ssh", "{doc}");
    }
}

#[test]
fn controller_socket_and_ssh_together_are_rejected() {
    let e = err("controller: {socket: /tmp/x.sock, ssh: newman@minion}");
    assert_eq!(e.path, "controller");
    assert!(e.reason.contains("only one"), "{e}");
}

#[test]
fn removed_credential_and_github_keys_point_at_the_user_secrets() {
    for key in ["credentials", "github"] {
        for doc in [format!("{key}:"), format!("{key}: {{a: 1}}")] {
            let e = err(&doc);
            assert_eq!(e.path, key, "{doc}: {e}");
            assert!(e.reason.contains("moor user secret set"), "{doc}: {e}");
        }
    }
}

#[test]
fn box_credentials_and_mcp_refs_may_not_name_a_secret_a_proxy_rule_injects() {
    let npm =
        "proxy: {rules: {npm: {hosts: [a.com], credential: npm-token, inject: {basic_user: u}}}}\n";
    let cases = [
        (
            "box_credentials: [a, claude]".to_owned(),
            "box_credentials[1]",
            "claude",
        ),
        (
            "box_credentials: [github]".to_owned(),
            "box_credentials[0]",
            "github-api",
        ),
        (
            format!("{npm}box_credentials: [npm-token]"),
            "box_credentials[0]",
            "npm",
        ),
        (
            "mcp_servers: {m: {command: x, args: ['${github}']}}".to_owned(),
            "mcp_servers.m.args[0]",
            "github-api",
        ),
        (
            "mcp_servers: {m: {url: 'http://h/${claude}'}}".to_owned(),
            "mcp_servers.m.url",
            "claude",
        ),
        (
            format!("{npm}mcp_servers: {{m: {{command: x, env: {{K: '${{npm-token}}'}}}}}}"),
            "mcp_servers.m.env.K",
            "npm",
        ),
    ];
    for (doc, path, rule) in &cases {
        let e = err(doc);
        assert_eq!(e.path, *path, "{doc}: {e}");
        let want = format!("the Proxy's {rule} rule injects");
        assert!(e.reason.contains(&want), "{doc}: {e}");
    }
    ok("box_credentials: [npm-token]\nmcp_servers: {m: {url: 'https://h.example', headers: {A: '${npm-token}'}}}");
}

#[test]
fn cache_volumes_are_on_per_provider_unless_turned_off() {
    let cfg = parse_user("u.yaml", "drivers:\n  a: {}\n  b: {cache_volumes: false}\n").unwrap();
    assert!(cfg.drivers.get("a").unwrap().cache_volumes());
    assert!(!cfg.drivers.get("b").unwrap().cache_volumes());
    assert!(DriverConfig::default().cache_volumes());
}

#[test]
fn launch_timeout_defaults_to_three_minutes_and_parses() {
    assert_eq!(ok("").box_.launch_timeout(), Duration::from_mins(3));
    assert_eq!(
        ok("box: {launch_timeout: 1m}").box_.launch_timeout(),
        Duration::from_mins(1)
    );
    assert_eq!(
        ok("box: {launch_timeout: 90s}").box_.launch_timeout(),
        Duration::from_secs(90)
    );
}

#[test]
fn a_launch_timeout_that_is_not_a_duration_is_refused() {
    let err = parse_user("user.yaml", "box: {launch_timeout: soon}").unwrap_err();
    assert!(err.to_string().contains("box.launch_timeout"), "{err}");
}

#[test]
fn surface_kind_parses() {
    assert_eq!(ok("surface: {kind: pty}").surface.kind, SurfaceKind::Pty);
    assert_eq!(
        ok("surface: {kind: herdr}").surface.kind,
        SurfaceKind::Herdr
    );
    err("surface: {kind: tmux}");
}
