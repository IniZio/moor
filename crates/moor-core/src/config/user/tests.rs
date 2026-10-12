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
    assert_eq!(c.provider_or_default(), "local-container");
    assert_eq!(c.github.identity, Identity::Auto);
    assert_eq!(BoxSize::default(), BoxSize::Small);
    assert_eq!(c.box_.size, None);
}

#[test]
fn provider_session_and_tools_parse() {
    let c = ok("box: {provider: static, size: large, max_boxes: 3, enforce_egress: true, sleep_after: 30m}\n\
         surface: {herdr: {session: work}}\n\
         tools: {crabbox: /opt/crabbox, git: git2}\n");
    assert_eq!(c.provider_or_default(), "static");
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
        "drivers: {crabbox: {max_boxes: 2}}",
        "dotfiles: {path: /abs, install: install.sh}",
        "dotfiles: {path: ~/dots}",
        "dotfiles: {path: /abs, install: ./i.sh}",
        "git: {name: N, email: e@example.com}",
        "agent: {model: haiku}",
        "egress: {allow: [Example.COM, '*.example.org']}",
        "state_dir: /var/moor",
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
    ];
    check_rejects(cases);
}

#[test]
fn rejected_values_with_paths() {
    let cases: &[(&str, &str)] = &[
        ("github: {identity: Bot}", "github.identity"),
        ("github: {identity: other}", "github.identity"),
        ("box: {size: huge}", "box.size"),
        ("box: {enforce_egress: 'yes'}", "box.enforce_egress"),
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
fn reports_rejection_names_the_key() {
    let e = err("reports: {backend: local}");
    assert_eq!(e.path, "reports");
    assert!(e.reason.contains("unknown field"), "{e}");
}
