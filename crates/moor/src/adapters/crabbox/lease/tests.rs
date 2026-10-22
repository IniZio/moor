use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};
use moor_core::seam::driver::{AcquireReq, BoxStep, Driver, ExecSpec, Released};
use moor_core::seam::proc::{Deadline, ProcSpec, Tool};

use super::CEILING;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{ChildReply, Reply, ScriptedProc};

const PROVIDERS: &str = r#"[{"provider":"pauser","features":["pause-resume"]}]"#;
const SSH_LINE: &str = "ssh -p 2222 user@1.2.3.4\n";
const WARMUP_OUT: &str = "leased cbx_1 slug=moor-7-ab12cd provider=pauser server=1 type=t \
ip=1.2.3.4\nready ssh=u@h:22 network=public workroot=/root/crabbox\n";
const SLUG: &str = "moor-7-ab12cd";

fn req() -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new("pauser"),
        box_id: BoxId::new(7),
        name: BoxName::parse_allow_bare("t7").unwrap(),
        class: None,
        nonce: "ab12cd".to_owned(),
    }
}

fn handle() -> BoxHandle {
    BoxHandle::new(format!("pauser/{SLUG}"))
}

fn scripted() -> Arc<ScriptedProc> {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(PROVIDERS.to_owned()),
    );
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    p.on(Tool::Crabbox, &["tunnel"], Reply::Ok(String::new()));
    p.on_child(
        Tool::Crabbox,
        &["tunnel"],
        ChildReply::Ready(r#"{"box":8080,"socket":"/s"}"#.to_owned()),
    );
    p.on(Tool::Crabbox, &[], Reply::Ok(String::new()));
    p.on(Tool::Ssh, &[], Reply::Ok(String::new()));
    p
}

fn leases(c: &Crabbox) -> PathBuf {
    fs::canonicalize(c.leases.as_ref().unwrap()).unwrap()
}

fn path() -> BoxPath {
    BoxPath::under("/w", "f").unwrap()
}

fn exec_spec() -> ExecSpec {
    ExecSpec {
        argv: vec!["true".to_owned()],
        env: Vec::new(),
        cwd: None,
        output: None,
    }
}

type Call = fn(&Crabbox);

/// Every Driver method that names a lease, as one call each.
const CALLS: [(&str, Call); 13] = [
    ("acquire", |c| drop(c.acquire(&req()))),
    ("release", |c| drop(c.release(&handle()))),
    ("workdir", |c| drop(c.workdir(&handle()))),
    ("exec", |c| {
        drop(c.exec(&handle(), &exec_spec(), Deadline(Duration::from_secs(1))));
    }),
    ("copy_out", |c| {
        drop(c.copy_out(&handle(), &path(), Path::new("/tmp/x")));
    }),
    ("stream_files", |c| {
        let names = ["f".to_owned()];
        let deadline = Deadline(Duration::from_secs(1));
        drop(c.stream_files(&handle(), &path(), &names, deadline, &mut |_, _| {}));
    }),
    ("peek_file", |c| drop(c.peek_file(&handle(), &path()))),
    ("put_file", |c| {
        drop(c.put_file(&handle(), &path(), b"x", 0o600));
    }),
    ("connect_argv", |c| drop(c.connect_argv(&handle()))),
    ("reverse_tunnel", |c| {
        drop(c.reverse_tunnel(&handle(), 8080, Path::new("/s")));
    }),
    ("batch", |c| {
        let steps = [BoxStep::Peek(path())];
        drop(c.batch(&handle(), &[], &steps, Deadline(Duration::from_secs(1))));
    }),
    ("sleep", |c| drop(c.sleep(&handle()))),
    ("wake", |c| drop(c.wake(&handle()))),
];

#[test]
fn every_lease_call_runs_from_the_lease_dir_under_a_git_ceiling() {
    for (name, call) in CALLS {
        let p = scripted();
        let c = Crabbox::fake(p.clone());
        call(&c);
        let root = leases(&c);
        let on_lease: Vec<_> = p
            .calls()
            .into_iter()
            .filter(|k| {
                k.tool == Tool::Crabbox && k.args.iter().any(|a| a == "--id" || a == "--slug")
            })
            .collect();
        assert!(
            !on_lease.is_empty(),
            "{name} made no crabbox call on the lease"
        );
        for k in on_lease {
            assert_eq!(k.cwd, Some(root.join(SLUG)), "{name}: {:?}", k.args);
            let ceiling = k.env.iter().find(|(key, _)| key == CEILING);
            assert_eq!(
                ceiling.map(|(_, v)| PathBuf::from(v)),
                Some(root.clone()),
                "{name}: {:?}",
                k.args
            );
        }
    }
}

#[test]
fn the_lease_dir_and_its_parent_are_private() {
    let c = Crabbox::fake(scripted());
    c.acquire(&req()).unwrap();
    let root = leases(&c);
    for dir in [root.join(SLUG), root] {
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
}

/// Runs `git rev-parse --show-toplevel` the way `spec` would run crabbox.
fn toplevel(spec: &ProcSpec) -> std::process::Output {
    let mut git = Command::new("git");
    git.args(["rev-parse", "--show-toplevel"])
        .current_dir(spec.cwd.as_ref().unwrap())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove(CEILING);
    git.envs(spec.env.extra.iter().map(|(k, v)| (k, v)));
    git.output().unwrap()
}

#[test]
fn a_state_dir_inside_a_git_repo_never_becomes_the_claim_root() {
    let repo = tempfile::tempdir().unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(init.success());
    let state = repo.path().join("state");
    let c = Crabbox::fake(scripted()).with_state_dir(Some(state), repo.path().join("s"));
    let spec = || {
        let spec = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(1)));
        c.at_lease(spec, &handle()).unwrap()
    };
    let mut bare = spec();
    bare.env.extra.retain(|(k, _)| k != CEILING);
    assert!(
        toplevel(&bare).status.success(),
        "the dir is inside the repo"
    );
    let out = toplevel(&spec());
    assert!(
        !out.status.success(),
        "git found {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn released(reply: Reply) -> (Result<Released, moor_core::seam::driver::DriverError>, bool) {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["stop"], reply);
    let c = Crabbox::fake(p);
    let dir = leases_dir_made(&c);
    let got = c.release(&handle());
    (got, dir.exists())
}

fn leases_dir_made(c: &Crabbox) -> PathBuf {
    let spec = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(1)));
    c.at_lease(spec, &handle()).unwrap().cwd.unwrap()
}

fn stop_fails(code: i32, stderr: &str) -> Reply {
    Reply::Fail {
        code,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn release_removes_the_lease_dir_once_the_lease_is_gone() {
    let (got, kept) = released(Reply::Ok(String::new()));
    assert_eq!(got, Ok(Released::Deleted));
    assert!(!kept);
    let (got, kept) = released(stop_fails(4, "lease not found\n"));
    assert_eq!(got, Ok(Released::AlreadyGone));
    assert!(!kept);
}

#[test]
fn a_failed_release_keeps_the_lease_dir_and_names_it_for_a_release_by_hand() {
    let (got, kept) = released(stop_fails(1, "boom"));
    assert!(got.is_err());
    assert!(kept);
    let c = Crabbox::fake(Arc::new(ScriptedProc::new()));
    let dir = leases_dir_made(&c);
    let how = c.manual_release(&handle()).unwrap();
    assert!(
        how.contains(&format!("--provider pauser --id {SLUG} --delete")),
        "{how}"
    );
    assert!(how.contains(&dir.display().to_string()), "{how}");
}

#[test]
fn a_handle_that_is_not_a_plain_name_has_no_lease_dir() {
    let c = Crabbox::fake(Arc::new(ScriptedProc::new()));
    for bad in ["p/../x", "p/.hidden", "p/", "p/a b", "p/a/b"] {
        assert!(c.lease_dir(&BoxHandle::new(bad)).is_err(), "{bad}");
    }
}

#[test]
fn without_a_state_dir_lease_calls_are_refused() {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone()).with_state_dir(None, PathBuf::from("/s"));
    assert!(c.release(&handle()).is_err());
    assert!(p
        .calls()
        .iter()
        .all(|k| k.args.first().is_none_or(|a| a != "stop")));
}

#[test]
fn a_state_dir_behind_a_symlink_inside_a_git_repo_never_becomes_the_claim_root() {
    let top = tempfile::tempdir().unwrap();
    let repo = top.path().join("repo");
    fs::create_dir_all(repo.join("state")).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(init.success());
    let link = top.path().join("state-link");
    std::os::unix::fs::symlink(repo.join("state"), &link).unwrap();
    let c = Crabbox::fake(scripted()).with_state_dir(Some(link), top.path().join("s"));
    let spec = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(1)));
    let spec = c.at_lease(spec, &handle()).unwrap();
    let cwd = spec.cwd.clone().unwrap();
    assert_eq!(
        cwd,
        fs::canonicalize(&cwd).unwrap(),
        "a walk that reads paths as text"
    );
    let ceiling = spec.env.extra.iter().find(|(k, _)| k == CEILING).unwrap();
    assert_eq!(Some(Path::new(&ceiling.1)), cwd.parent());
    let out = toplevel(&spec);
    assert!(
        !out.status.success(),
        "git found {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_failed_acquire_removes_the_lease_dir_it_made() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(PROVIDERS.to_owned()),
    );
    p.on(Tool::Crabbox, &["warmup"], stop_fails(1, "no capacity"));
    p.on(Tool::Crabbox, &[], Reply::Ok(String::new()));
    let c = Crabbox::fake(p);
    assert!(c.acquire(&req()).is_err());
    let dir = c.lease_dir(&handle()).unwrap();
    assert!(!dir.exists(), "{} kept", dir.display());
    assert!(c.leases.as_ref().unwrap().exists());
}

#[test]
fn a_failed_acquire_keeps_a_lease_dir_that_was_already_there() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(PROVIDERS.to_owned()),
    );
    p.on(Tool::Crabbox, &["warmup"], stop_fails(1, "no capacity"));
    p.on(Tool::Crabbox, &[], Reply::Ok(String::new()));
    let c = Crabbox::fake(p);
    let dir = leases_dir_made(&c);
    assert!(c.acquire(&req()).is_err());
    assert!(dir.exists(), "{} removed", dir.display());
}
