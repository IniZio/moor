//! `pull`: a file on the machine into the Box's work tree.

use serde_json::json;

use crate::rig::Rig;

fn machine_file(rig: &Rig, rel: &str, contents: &str) {
    let path = rig.copy().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn a_pulled_file_reaches_the_controller_as_a_box_put() {
    let rig = Rig::start();
    machine_file(&rig, "logs/build.log", "built ok");
    let got = rig
        .call(
            "pull",
            json!({"path": "logs/build.log", "dest": "out/build.log"}),
        )
        .unwrap();
    assert_eq!(got, json!({"dest": "out/build.log", "bytes": 8}));
    let puts = rig.puts.lock().unwrap();
    assert_eq!(puts.len(), 1);
    let (req, bytes) = &puts[0];
    assert_eq!(req["id"], 7);
    assert_eq!(req["dest"], "out/build.log");
    assert_eq!(bytes, b"built ok");
    assert!(!std::path::Path::new(req["src"].as_str().unwrap()).exists());
}

#[test]
fn an_absolute_machine_path_is_pulled_as_is() {
    let rig = Rig::start();
    let shot = rig.remote.join("shots/1.png");
    std::fs::create_dir_all(shot.parent().unwrap()).unwrap();
    std::fs::write(&shot, "png").unwrap();
    let args = json!({"path": shot.display().to_string(), "dest": "1.png"});
    rig.call("pull", args).unwrap();
    assert_eq!(rig.puts.lock().unwrap()[0].1, b"png");
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
    assert!(rig.puts.lock().unwrap().is_empty());
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
    assert!(rig.puts.lock().unwrap().is_empty());
}
