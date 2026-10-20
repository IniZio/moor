//! Guards the dependency rule: the fronts and their socket client never link the `moor` crate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a failed setup step should abort the test"
)]

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

const RULE: &str = "moor-web, moor-slack and moor-client must never depend on the moor crate";

/// Packages that talk to the controller over its socket and so must not link `moor`.
const SOCKET_CRATES: [&str; 3] = ["moor-web", "moor-slack", "moor-client"];

/// Walks the resolved graph from the package named `root_name`, over every
/// dependency kind. Returns the package names from root to `moor` if reachable.
fn reaches_moor(metadata: &Value, root_name: &str) -> Option<Vec<String>> {
    let names: HashMap<&str, &str> = metadata
        .get("packages")?
        .as_array()?
        .iter()
        .filter_map(|p| Some((p.get("id")?.as_str()?, p.get("name")?.as_str()?)))
        .collect();
    let deps: HashMap<&str, Vec<&str>> = metadata
        .get("resolve")?
        .get("nodes")?
        .as_array()?
        .iter()
        .filter_map(|n| {
            let id = n.get("id")?.as_str()?;
            let next = n
                .get("deps")?
                .as_array()?
                .iter()
                .filter_map(|d| d.get("pkg")?.as_str())
                .collect();
            Some((id, next))
        })
        .collect();
    let root = names
        .iter()
        .find_map(|(id, name)| (*name == root_name).then_some(*id))?;

    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        if names.get(id) == Some(&"moor") {
            let mut path = vec![id];
            let mut cur = id;
            while let Some(&p) = parent.get(cur) {
                path.push(p);
                cur = p;
            }
            path.reverse();
            return Some(
                path.iter()
                    .map(|p| (*names.get(p).unwrap_or(p)).to_owned())
                    .collect(),
            );
        }
        for &next in deps.get(id).into_iter().flatten() {
            if next != root && !parent.contains_key(next) {
                parent.insert(next, id);
                queue.push_back(next);
            }
        }
    }
    None
}

/// Runs `cargo metadata` on `manifest` and parses the JSON. Not `--offline`: metadata needs
/// every package in `Cargo.lock`, and a fresh `CARGO_HOME` only has its own platform's.
fn cargo_metadata(manifest: &Path, locked: bool) -> Value {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(manifest);
    if locked {
        cmd.arg("--locked");
    }
    let out = cmd.output().expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse cargo metadata")
}

/// Builds metadata where each `(from, key, to)` edge is a dependency of
/// package `from` on package `to` under the dependency key `key`.
fn fixture(packages: &[&str], edges: &[(&str, &str, &str)]) -> Value {
    let pkgs: Vec<Value> = packages
        .iter()
        .map(|n| json!({ "id": format!("id-{n}"), "name": n }))
        .collect();
    let nodes: Vec<Value> = packages
        .iter()
        .map(|n| {
            let deps: Vec<Value> = edges
                .iter()
                .filter(|(from, _, _)| from == n)
                .map(|(_, key, to)| json!({ "name": key, "pkg": format!("id-{to}") }))
                .collect();
            json!({ "id": format!("id-{n}"), "deps": deps })
        })
        .collect();
    json!({ "packages": pkgs, "resolve": { "nodes": nodes } })
}

#[test]
fn walk_flags_direct_dependency() {
    let m = fixture(&["web", "moor"], &[("web", "moor", "moor")]);
    assert_eq!(
        reaches_moor(&m, "web"),
        Some(vec!["web".into(), "moor".into()])
    );
}

#[test]
fn walk_flags_transitive_dependency() {
    let m = fixture(
        &["web", "x", "moor"],
        &[("web", "x", "x"), ("x", "moor", "moor")],
    );
    let path = reaches_moor(&m, "web").expect(RULE);
    assert_eq!(path, ["web", "x", "moor"]);
}

#[test]
fn walk_flags_renamed_dependency() {
    let m = fixture(&["web", "moor"], &[("web", "renamed", "moor")]);
    assert!(reaches_moor(&m, "web").is_some(), "{RULE}");
}

#[test]
fn walk_spares_moor_core() {
    let m = fixture(
        &["web", "moor-core", "moor"],
        &[
            ("web", "moor-core", "moor-core"),
            ("moor", "moor-core", "moor-core"),
        ],
    );
    assert_eq!(reaches_moor(&m, "web"), None);
}

#[test]
fn walk_survives_cycles() {
    let m = fixture(
        &["web", "a", "b"],
        &[("web", "a", "a"), ("a", "b", "b"), ("b", "a", "a")],
    );
    assert_eq!(reaches_moor(&m, "web"), None);
}

#[test]
fn socket_crates_do_not_depend_on_moor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let metadata = cargo_metadata(&root, true);
    for name in SOCKET_CRATES {
        if let Some(path) = reaches_moor(&metadata, name) {
            panic!("{RULE}: {}", path.join(" -> "));
        }
    }
}

const MOOR_MANIFEST: &str = "[package]\nname = \"moor\"\nversion = \"0.0.0\"\nedition = \"2021\"\n";

/// Writes a workspace of a `moor` lib and a `web` lib whose manifest
/// carries `web_deps` after the `[package]` header.
fn write_workspace(dir: &Path, web_deps: &str) {
    let files = [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"moor\", \"web\"]\nresolver = \"2\"\n\n\
             [workspace.dependencies]\nmoor = { path = \"moor\" }\n"
                .to_owned(),
        ),
        ("moor/Cargo.toml", MOOR_MANIFEST.to_owned()),
        ("moor/src/lib.rs", String::new()),
        (
            "web/Cargo.toml",
            format!(
                "[package]\nname = \"web\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{web_deps}"
            ),
        ),
        ("web/src/lib.rs", String::new()),
    ];
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write file");
    }
}

fn manifest_reaches_moor(web_deps: &str) -> bool {
    let dir = tempfile::tempdir().expect("tempdir");
    write_workspace(dir.path(), web_deps);
    let metadata = cargo_metadata(&dir.path().join("Cargo.toml"), false);
    reaches_moor(&metadata, "web").is_some()
}

#[test]
fn every_manifest_form_of_moor_is_detected() {
    let forms = [
        ("table", "[dependencies.moor]\npath = \"../moor\"\n"),
        (
            "dotted workspace",
            "[dependencies]\nmoor.workspace = true\n",
        ),
        ("dotted path", "[dependencies]\nmoor.path = \"../moor\"\n"),
        (
            "renamed table",
            "[dependencies.foo]\npath = \"../moor\"\npackage = \"moor\"\n",
        ),
        (
            "single-quoted package",
            "[dependencies]\nfoo = { path = \"../moor\", package = 'moor' }\n",
        ),
        (
            "quoted key",
            "[dependencies]\n\"moor\" = { path = \"../moor\" }\n",
        ),
        (
            "dev dependency",
            "[dev-dependencies]\nmoor = { path = \"../moor\" }\n",
        ),
    ];
    for (label, deps) in forms {
        assert!(manifest_reaches_moor(deps), "{RULE}: missed form `{label}`");
    }
    assert!(
        !manifest_reaches_moor("[dependencies]\n"),
        "control manifest must not reach moor"
    );
}
