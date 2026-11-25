//! `moor repo config get` and `set`.

use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq, RepoFile};
use moor_core::seam::forge::ForgeRepo;
use moor_core::Request;

use super::{req, run};

#[test]
fn get_names_the_repo_as_owner_slash_name_or_a_url() {
    let want = Request::RepoConfigGet(RepoConfigGetReq {
        file: RepoFile::Config,
        identity: None,
        repo: ForgeRepo::new("o", "n"),
    });
    assert_eq!(req(&["repo", "config", "get", "o/n"]), want);
    assert_eq!(
        req(&["repo", "config", "get", "https://github.com/o/n"]),
        want
    );
    assert!(run(&["repo", "config", "get"]).is_err());
}

#[test]
fn rules_get_and_set_name_the_rules_file() {
    let want = Request::RepoConfigGet(RepoConfigGetReq {
        file: RepoFile::Rules,
        identity: None,
        repo: ForgeRepo::new("o", "n"),
    });
    assert_eq!(req(&["repo", "rules", "get", "o/n"]), want);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rules.yaml");
    std::fs::write(&file, "rules: []\n").unwrap();
    let set = req(&["repo", "rules", "set", "o/n", "-f", file.to_str().unwrap()]);
    let Request::RepoConfigSet(set) = set else {
        unreachable!("{set:?}")
    };
    assert_eq!(
        (set.file, set.text.as_str()),
        (RepoFile::Rules, "rules: []\n")
    );
}

#[test]
fn set_reads_the_file_and_carries_the_revision_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("repo.yaml");
    std::fs::write(&file, "setup: [make]\n").unwrap();
    let file = file.to_str().unwrap();
    assert_eq!(
        req(&[
            "repo",
            "config",
            "set",
            "o/n",
            "--file",
            file,
            "--expected-revision",
            "3"
        ]),
        Request::RepoConfigSet(RepoConfigSetReq {
            file: RepoFile::Config,
            identity: None,
            repo: ForgeRepo::new("o", "n"),
            text: "setup: [make]\n".to_owned(),
            expected_revision: Some(3),
        })
    );
    let plain = match req(&["repo", "config", "set", "o/n", "-f", file]) {
        Request::RepoConfigSet(r) => Some(r.expected_revision),
        _ => None,
    };
    assert_eq!(plain, Some(None));
}

#[test]
fn set_refuses_a_file_it_cannot_read_or_that_is_not_text() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.yaml");
    assert!(run(&[
        "repo",
        "config",
        "set",
        "o/n",
        "-f",
        missing.to_str().unwrap()
    ])
    .is_err());
    let binary = dir.path().join("bin.yaml");
    std::fs::write(&binary, [0xff, 0xfe]).unwrap();
    assert!(run(&[
        "repo",
        "config",
        "set",
        "o/n",
        "-f",
        binary.to_str().unwrap()
    ])
    .is_err());
}
