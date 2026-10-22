//! A start names its repo `OWNER/NAME` and reads it through the forge before any machine exists.

use moor_core::request::BoxTarget;
use moor_core::seam::forge::{ForgeError, RepoAt};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, CloseReq, ErrorKind, RunReq};

use super::{repo, repo_req, run, script_base_yaml, world, World, BASE};
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

const YAML: &str = "setup: [from-forge]\n";

/// The token the test requester holds for `github`.
const TOKEN: &str = "github-test-value";

fn started() -> (World, BoxId) {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(YAML));
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    (w, BoxId::new(v.boxed.id))
}

#[test]
fn a_start_stores_the_forge_repo_base_config_and_author_then_acquires() {
    let (w, id) = started();
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let stored = row.repo.expect("repo");
    assert_eq!(stored.repo, repo());
    assert_eq!(stored.moor_yaml.as_deref(), Some(YAML));
    assert_eq!(stored.author.name, "Ann Dev");
    assert_eq!(stored.author.email, "1+ann@users.noreply.github.com");
    assert_eq!(row.base.as_deref(), Some(BASE));
    assert!(w.driver.calls().iter().any(|c| c == "acquire"));
    let reads = w.forge.reads();
    assert_eq!(reads.len(), 1, "{reads:?}");
    let read = reads.first().unwrap();
    assert_eq!(
        (read.base.as_deref(), read.branch.as_str()),
        (None, "moor/demo")
    );
    assert_eq!(read.token, TOKEN);
}

#[test]
fn the_setup_lines_come_from_the_forge_copy() {
    let (w, _) = started();
    let ran = w
        .driver
        .execs()
        .iter()
        .any(|(_, spec, _)| spec.argv.iter().any(|a| a.contains("from-forge")));
    assert!(ran, "{:?}", w.driver.execs());
}

#[test]
fn a_repo_the_account_cannot_see_is_refused_before_any_machine() {
    let w = world(FakeDriver::new());
    let err = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.error.to_string(),
        "repo o/n is not visible to your GitHub account"
    );
    assert!(w.driver.calls().is_empty(), "{:?}", w.driver.calls());
    assert!(w.surface.calls().is_empty());
    let name = repo_req("demo").name;
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_box_by_name(&name)
        .unwrap()
        .is_none());
}

#[test]
fn a_box_branch_already_on_the_forge_is_refused_unless_base_names_it() {
    let w = world(FakeDriver::new());
    let taken = RepoAt {
        branch_tip: Some(moor_core::Sha::parse("def456").unwrap()),
        ..repo_at(BASE, None)
    };
    w.forge.answer(repo(), Ok(taken));
    let err = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.error.to_string(),
        "branch moor/demo already exists on o/n"
    );
    assert!(
        err.hints.iter().any(|h| h.contains("--base moor/demo")),
        "{err:?}"
    );
    assert!(w.driver.calls().is_empty());
    let mut again = repo_req("demo");
    again.base = Some("moor/demo".into());
    run(&w.cx, &again).unwrap();
    assert!(w.driver.calls().iter().any(|c| c == "acquire"));
}

#[test]
fn only_a_base_of_exactly_the_box_branch_continues_a_taken_branch() {
    for base in ["refs/heads/moor/demo", "origin/moor/demo", "moor/demo~0", "moor/demo/", "Moor/demo"] {
        let w = world(FakeDriver::new());
        let taken = RepoAt {
            branch_tip: Some(moor_core::Sha::parse("def456").unwrap()),
            ..repo_at(BASE, None)
        };
        w.forge.answer(repo(), Ok(taken));
        let mut req = repo_req("demo");
        req.base = Some(base.into());
        let err = run(&w.cx, &req).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused, "{base}");
        assert!(w.driver.calls().is_empty(), "{base}");
    }
}

#[test]
fn an_account_that_cannot_push_starts_with_a_warning() {
    let w = world(FakeDriver::new());
    let read_only = RepoAt {
        can_push: false,
        ..repo_at(BASE, None)
    };
    w.forge.answer(repo(), Ok(read_only));
    run(&w.cx, &repo_req("demo")).unwrap();
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().any(|w| w.code == "repo_read_only"),
        "{warnings:?}"
    );
}

#[test]
fn an_unknown_base_is_a_usage_error() {
    let w = world(FakeDriver::new());
    w.forge
        .answer(repo(), Err(ForgeError::UnknownBase("nope".into())));
    let mut req = repo_req("demo");
    req.base = Some("nope".into());
    let err = run(&w.cx, &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(w.driver.calls().is_empty());
    assert!(w.surface.calls().is_empty());
    let rows: i64 = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM boxes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn the_token_never_reaches_a_reply_event_or_error() {
    for answer in [
        Err(ForgeError::NotVisible),
        Err(ForgeError::Rejected),
        Err(ForgeError::Unavailable("timeout".into())),
        Err(ForgeError::Unreadable(".moor.yaml")),
        Ok(repo_at(BASE, Some(YAML))),
    ] {
        let w = world(FakeDriver::new());
        w.forge.answer(repo(), answer.clone());
        let out = match run(&w.cx, &repo_req("demo")) {
            Ok(v) => serde_json::to_string(&v).unwrap(),
            Err(e) => format!("{} {:?} {:?}", e.error, e.hints, e.value),
        };
        let warnings = format!("{:?}", w.cx.take_warnings());
        let store = w.cx.store().unwrap();
        let events: String = store
            .open_boxes()
            .unwrap()
            .iter()
            .flat_map(|b| store.events(b.id, 100).unwrap())
            .map(|e| e.payload.to_string())
            .collect();
        let logs: String = walk(w.dir.path());
        for text in [&out, &warnings, &events, &logs] {
            assert!(!text.contains(TOKEN), "{answer:?}: {text}");
        }
    }
}

/// Every text file under `dir`, concatenated; the state dir's logs and database included.
fn walk(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&walk(&path));
        } else if path.file_name().is_some_and(|n| n != "vault.key") {
            out.push_str(&String::from_utf8_lossy(
                &std::fs::read(&path).unwrap_or_default(),
            ));
        }
    }
    out
}

#[test]
fn a_start_a_run_and_a_close_spawn_no_host_git() {
    let (w, _) = started();
    let name = repo_req("demo").name;
    let ran = RunReq {
        box_name: name.clone(),
        record: None,
        argv: vec!["true".to_owned()],
        inherit_output: false,
    };
    crate::ops::run::run(&w.cx, &ran).unwrap();
    let close = CloseReq {
        target: BoxTarget::Name(name),
        force: true,
    };
    crate::ops::close::run(&w.cx, &close).unwrap();
    let git: Vec<_> = w
        .proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Git)
        .map(|c| c.args)
        .collect();
    assert!(git.is_empty(), "{git:?}");
}
