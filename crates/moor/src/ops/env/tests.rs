//! Tests for `env` and its item assembly.

use std::path::Path;
use std::sync::Arc;

use moor_core::config::{Layer, UserConfig};
use moor_core::seam::forge::{ForgeError, ForgeRepo, RepoAt, RepoFiles};
use moor_core::view::env::{EnvKind, EnvView};
use moor_core::{EnvReq, ErrorKind};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// Context acting as the test requester, with user config `config`, over `forge`.
fn cx_over(dir: &Path, config: UserConfig, forge: FakeForge) -> Cx {
    Cx::fake_with(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        config,
    )
    .with_forge(Arc::new(forge))
}

/// Context with default user config.
fn cx(dir: &Path) -> Cx {
    cx_over(dir, UserConfig::default(), FakeForge::new())
}

fn on() -> ForgeRepo {
    ForgeRepo::new("o", "n")
}

/// `env` of `o/n` whose forge answers `at`.
fn env_of(at: RepoAt) -> (tempfile::TempDir, EnvView) {
    let d = tempfile::tempdir().unwrap();
    let cx = cx_over(
        d.path(),
        UserConfig::default(),
        FakeForge::new().with_read(on(), Ok(at)),
    );
    let req = EnvReq {
        repo: Some(on()),
        base: None,
    };
    let v = run(&cx, &req).expect("env ok");
    (d, v)
}

/// `env` of `o/n` holding `.moor.yaml` text `yaml` at its base.
fn env_with(yaml: &str) -> (tempfile::TempDir, EnvView) {
    env_of(repo_at("abc123", Some(yaml)))
}

/// `env` of `o/n` holding `files` at its base.
fn env_files(files: RepoFiles) -> EnvView {
    env_of(RepoAt {
        files,
        ..repo_at("abc123", None)
    })
    .1
}

fn kinds(v: &EnvView) -> Vec<EnvKind> {
    v.rows.iter().map(|i| i.kind).collect()
}

#[test]
fn no_repo_shows_the_user_layer_and_reads_no_forge() {
    let d = tempfile::tempdir().unwrap();
    let forge = Arc::new(FakeForge::new());
    let cx = cx(d.path()).with_forge(forge.clone());
    let v = run(
        &cx,
        &EnvReq {
            repo: None,
            base: None,
        },
    )
    .unwrap();
    assert!(v.rows.iter().any(|r| r.name == "provider"));
    assert!(forge.reads().is_empty());
}

#[test]
fn a_repo_is_read_at_the_base_with_the_requesters_token() {
    let d = tempfile::tempdir().unwrap();
    let forge = Arc::new(FakeForge::new().with_read(on(), Ok(repo_at("abc123", None))));
    let cx = cx(d.path()).with_forge(forge.clone());
    let req = EnvReq {
        repo: Some(on()),
        base: Some("v1".into()),
    };
    run(&cx, &req).unwrap();
    let reads = forge.reads();
    assert_eq!(reads.len(), 1);
    let read = reads.first().unwrap();
    assert_eq!(read.base.as_deref(), Some("v1"));
    assert_eq!(read.token, "github-test-value");
}

#[test]
fn a_repo_the_account_cannot_see_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let forge = FakeForge::new().with_read(on(), Err(ForgeError::NotVisible));
    let cx = cx_over(d.path(), UserConfig::default(), forge);
    let req = EnvReq {
        repo: Some(on()),
        base: None,
    };
    let err = run(&cx, &req).expect_err("refused");
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("o/n is not visible"),
        "{err:?}"
    );
}

#[test]
fn item_order_is_fixed() {
    let (_d, v) = env_with("setup: [make]\n");
    let k = kinds(&v);
    let pos = |kind| k.iter().position(|x| *x == kind).unwrap();
    assert!(pos(EnvKind::Setup) < pos(EnvKind::Box));
    let boxes: Vec<_> = v
        .rows
        .iter()
        .filter(|i| i.kind == EnvKind::Box)
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(boxes, ["provider", "size", "gh"]);
}

#[test]
fn a_repo_credential_is_listed_and_left_for_start_to_check() {
    let (_d, v) = env_with("box_credentials: [token]\n");
    let row = v
        .rows
        .iter()
        .find(|r| r.name == "token")
        .expect("token row");
    assert_eq!(row.note, "secret of the Box's user");
    assert!(v.credentials.iter().all(|c| c.which != "token"));
    assert!(v.rows.iter().any(|r| r.name == "size"));
}

#[test]
fn repo_mcp_needs_approval() {
    let v = env_files(RepoFiles {
        mcp_json: Some(r#"{"mcpServers":{"a":{},"b":{}}}"#.into()),
        claude_settings: Some(r#"{"enabledMcpjsonServers":["a"]}"#.into()),
        moor_yaml: None,
    });
    let bad: Vec<_> = v
        .credentials
        .iter()
        .filter(|c| c.kind == EnvKind::Mcp && !c.ok)
        .map(|c| c.which.as_str())
        .collect();
    assert_eq!(bad, ["b"]);
}

#[test]
fn invalid_mcp_json_is_one_failed_item() {
    let v = env_files(RepoFiles {
        mcp_json: Some("{".into()),
        ..RepoFiles::default()
    });
    let m: Vec<_> = v
        .credentials
        .iter()
        .filter(|c| c.kind == EnvKind::Mcp)
        .collect();
    assert_eq!(m.len(), 1);
    assert!(m.first().is_some_and(|c| !c.ok));
}

/// Identity rows as `(name, value, source)`.
fn identity(v: &EnvView) -> Vec<(&str, &str, Layer)> {
    v.rows
        .iter()
        .filter(|r| r.kind == EnvKind::Identity)
        .map(|r| (r.name.as_str(), r.note.as_str(), r.source))
        .collect()
}

#[test]
fn identity_rows_come_from_the_forge_account() {
    let (_d, v) = env_of(repo_at("abc123", None));
    assert_eq!(
        identity(&v),
        [
            ("user.name", "Ann Dev", Layer::Forge),
            ("user.email", "1+ann@users.noreply.github.com", Layer::Forge)
        ]
    );
}

#[test]
fn no_repo_adds_no_identity_rows() {
    let d = tempfile::tempdir().unwrap();
    let v = run(
        &cx(d.path()),
        &EnvReq {
            repo: None,
            base: None,
        },
    )
    .unwrap();
    assert!(identity(&v).is_empty());
}

#[test]
fn a_config_naming_no_provider_shows_the_provider_as_unset() {
    let d = tempfile::tempdir().unwrap();
    let cx = Cx::fake_without_nodes(
        d.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let v = run(
        &cx,
        &EnvReq {
            repo: None,
            base: None,
        },
    )
    .unwrap();
    assert_eq!(v.provider.name, None);
    let row = v.rows.iter().find(|r| r.name == "provider").unwrap();
    assert_eq!(row.note, "unset");
}

#[test]
fn cache_rows_show_names_and_paths_and_never_a_key() {
    let (_d, v) = env_with("cache: {presets: [npm, yarn, pnpm, bun, docker, podman, dart, rust, go], volumes: [{name: sdk, path: /opt/sdk}]}");
    let rows: Vec<(&str, Layer, &str)> = v
        .rows
        .iter()
        .filter(|r| r.kind == EnvKind::Cache)
        .map(|r| (r.name.as_str(), r.source, r.note.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "gradle",
                Layer::Default,
                "~/.gradle/caches, ~/.gradle/wrapper"
            ),
            ("worktree", Layer::Default, "~/moor/repos/n"),
            ("sdk", Layer::Repo, "/opt/sdk"),
        ]
    );
    let json = serde_json::to_string(&v).unwrap();
    assert!(!json.contains("key") && !json.contains("group"), "{json}");
}

#[test]
fn a_box_without_a_repo_lists_no_presets() {
    let d = tempfile::tempdir().unwrap();
    let v = run(
        &cx(d.path()),
        &EnvReq {
            repo: None,
            base: None,
        },
    )
    .unwrap();
    assert!(!v.rows.iter().any(|r| r.kind == EnvKind::Cache));
}
