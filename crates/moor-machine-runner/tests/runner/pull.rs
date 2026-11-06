//! `pull`: a file on the machine into the Box's work tree.

use serde_json::json;

use crate::rig::Rig;

fn machine_file(rig: &Rig, rel: &str, contents: &str) {
    let path = rig.copy().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn a_pulled_file_lands_in_the_box_by_rsync_naming_no_path_of_the_runner() {
    let rig = Rig::start();
    machine_file(&rig, "logs/build.log", "built ok");
    let got = rig
        .call(
            "pull",
            json!({"path": "logs/build.log", "dest": "out/my build.log"}),
        )
        .unwrap();
    assert_eq!(got, json!({"dest": "out/my build.log", "bytes": 8}));
    let landed = rig.tree.join("out/my build.log");
    assert_eq!(std::fs::read_to_string(&landed).unwrap(), "built ok");
    let mode = std::os::unix::fs::PermissionsExt::mode(&landed.metadata().unwrap().permissions());
    assert_eq!(mode & 0o777, 0o644);
    let rsyncs = rig.rsyncs.lock().unwrap().clone();
    assert_eq!(rsyncs.len(), 1);
    assert_eq!(rsyncs[0]["box"], 7);
    assert_eq!(rsyncs[0]["argv"][1], "--server");
    let staged = std::fs::read_dir(rig.state().join("pulls"))
        .unwrap()
        .count();
    assert_eq!(staged, 0, "the staging file is gone");
}

#[test]
fn an_absolute_path_in_the_box_copy_is_pulled_as_is() {
    let rig = Rig::start();
    let shot = rig.copy().join("shots/1.png");
    std::fs::create_dir_all(shot.parent().unwrap()).unwrap();
    std::fs::write(&shot, "png").unwrap();
    let args = json!({"path": shot.display().to_string(), "dest": "1.png"});
    rig.call("pull", args).unwrap();
    assert_eq!(
        std::fs::read_to_string(rig.tree.join("1.png")).unwrap(),
        "png"
    );
}

#[test]
fn a_bad_dest_is_refused_and_nothing_is_sent() {
    let rig = Rig::start();
    machine_file(&rig, "a.txt", "a");
    for bad in ["", "/etc/x", "../x", "a/../../x"] {
        let err = rig
            .call("pull", json!({"path": "a.txt", "dest": bad}))
            .unwrap_err();
        assert!(err.contains("dest"), "{bad:?}: {err}");
    }
    assert_eq!(rig.puts(), 0);
}

#[test]
fn a_missing_or_directory_machine_path_is_refused_and_nothing_is_sent() {
    let rig = Rig::start();
    machine_file(&rig, "dir/a.txt", "a");
    for path in ["nope.txt", "dir"] {
        let err = rig
            .call("pull", json!({"path": path, "dest": "x"}))
            .unwrap_err();
        assert!(!err.is_empty(), "{path}");
    }
    assert_eq!(rig.puts(), 0);
}
