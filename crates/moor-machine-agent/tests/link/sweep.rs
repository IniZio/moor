//! Sweeps and removes of closed Boxes' folders over the link.

use super::*;

/// The sweep answer in `reply`.
pub(super) fn swept(reply: Reply) -> moor_client::link::sweep::Swept {
    match reply {
        Reply::Swept(swept) => swept,
        other => panic!("not swept: {other:?}"),
    }
}

fn dirs(home: &Path, names: &[&str]) {
    for name in names {
        std::fs::create_dir_all(home.join(name)).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_removes_the_folders_of_boxes_not_kept_and_nothing_else() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let (outside, h) = (tempfile::tempdir().unwrap(), home.path());
    std::fs::write(outside.path().join("precious"), "x").unwrap();
    dirs(
        h,
        &[
            "moor/3",
            "moor/4",
            "moor/5",
            "moor-state/3",
            "moor-state/5",
            "moor-state/6",
        ],
    );
    dirs(
        h,
        &[
            "moor/07",
            "moor/0",
            "moor/7.bak",
            "moor/notes",
            "moor-state/x",
        ],
    );
    std::fs::write(h.join("moor/9"), "a file named like a Box").unwrap();
    std::os::unix::fs::symlink(outside.path(), h.join("moor/8")).unwrap();
    std::os::unix::fs::symlink(outside.path(), h.join("moor-state/8")).unwrap();
    std::os::unix::fs::symlink(outside.path(), h.join("moor/3/escape")).unwrap();
    let _agent = linked(&w, h).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": [4, 5]})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![3, 6], vec![]));
    for gone in ["moor/3", "moor-state/3", "moor-state/6"] {
        assert!(!h.join(gone).exists(), "{gone}");
    }
    for stays in [
        "moor/4",
        "moor/5",
        "moor-state/5",
        "moor/07",
        "moor/0",
        "moor/7.bak",
        "moor/notes",
        "moor-state/x",
        "moor/9",
    ] {
        assert!(h.join(stays).exists(), "{stays}");
    }
    for link in ["moor/8", "moor-state/8"] {
        assert!(h.join(link).is_symlink(), "{link}");
    }
    assert!(
        outside.path().join("precious").is_file(),
        "followed a symlink"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_does_not_enter_a_folder_root_that_is_a_symlink() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let (outside, h) = (tempfile::tempdir().unwrap(), home.path());
    dirs(outside.path(), &["3", "4"]);
    std::os::unix::fs::symlink(outside.path(), h.join("moor")).unwrap();
    let _agent = linked(&w, h).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![], vec![]));
    assert!(outside.path().join("3").is_dir() && outside.path().join("4").is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_runs_the_release_hook_in_the_box_dir_before_removing_it() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path().canonicalize().unwrap();
    dirs(&h, &["moor/3", "moor-state/3"]);
    std::fs::write(h.join("moor/3/marker"), "x").unwrap();
    hook_file(
        &h.join("hooks/release"),
        "#!/bin/sh\necho \"$MOOR_BOX $PWD\" >> \"$HOME/released\"\nls marker >> \"$HOME/released\"\n",
    );
    let _agent = linked(&w, &h).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![3], vec![]));
    let log = std::fs::read_to_string(h.join("released")).unwrap();
    assert_eq!(log, format!("3 {}/moor/3\nmarker\n", h.display()));
    assert!(!h.join("moor/3").exists() && !h.join("moor-state/3").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_release_hook_keeps_that_box_folders_and_the_sweep_goes_on() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    dirs(h, &["moor/3", "moor-state/3", "moor/4"]);
    hook_file(
        &h.join("hooks/release"),
        "#!/bin/sh\n[ \"$MOOR_BOX\" = 3 ] && { echo cannot stop it; exit 1; }\nexit 0\n",
    );
    let _agent = linked(&w, h).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!(swept.removed, vec![4]);
    assert_eq!(swept.errors.len(), 1, "{swept:?}");
    assert_eq!(swept.errors[0].box_id, 3);
    assert_eq!(
        swept.errors[0].reason,
        "release exited Some(1): cannot stop it"
    );
    assert!(h.join("moor/3").is_dir() && h.join("moor-state/3").is_dir());
    assert!(!h.join("moor/4").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_is_served_without_allow_exec_and_noted_in_the_status() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    dirs(home.path(), &["moor/3"]);
    let _agent = linked_as(&w, home.path(), None, false).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!(swept.removed, vec![3]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let Reply::Status(status) = ask(&w, &json!({"op": "status"})).await.1 else {
            panic!("not a status");
        };
        if status
            .recent
            .iter()
            .any(|r| r.op == "sweep" && r.error.is_none())
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never noted: {:?}",
            status.recent
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_keeps_the_folders_of_a_box_younger_than_its_grace() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    dirs(h, &["moor/3", "moor-state/3"]);
    let machine = moor_machine_agent::Machine {
        home: h.to_owned(),
        shell: "/bin/sh".into(),
        hooks: h.join("hooks"),
        pool: None,
        allow_exec: false,
        sweep_grace: Duration::from_secs(3600),
    };
    let _agent = linked_machine(&w, machine).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![], vec![]));
    assert!(h.join("moor/3").is_dir() && h.join("moor-state/3").is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remove_takes_one_box_folders_without_its_hook_or_allow_exec() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    dirs(h, &["moor/3", "moor-state/3", "moor/4"]);
    hook_file(
        &h.join("hooks/release"),
        "#!/bin/sh\ntouch \"$HOME/released\"\n",
    );
    let _agent = linked_as(&w, h, None, false).await;
    let swept = swept(ask(&w, &json!({"op": "remove", "box_id": 3})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![3], vec![]));
    assert!(!h.join("moor/3").exists() && !h.join("moor-state/3").exists());
    assert!(h.join("moor/4").is_dir(), "another Box");
    assert!(!h.join("released").exists(), "ran the release hook");
}
