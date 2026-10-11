use super::*;

fn world() -> World {
    World::new("/bin/true")
}

#[test]
fn layout_and_env() {
    let w = world();
    assert!(w
        .root()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("parity-"));
    for t in TOOLS {
        assert!(fs::symlink_metadata(w.tool_path(t))
            .unwrap()
            .file_type()
            .is_symlink());
    }
    for d in [
        "box/work", "box/home", "box/tmp", "state", "fakes", "tmp", "trees", "repos",
    ] {
        assert!(w.root().join(d).is_dir(), "{d}");
    }
    let mode = fs::metadata(w.home().join(".config"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(fs::read_to_string(w.home().join(".gitconfig"))
        .unwrap()
        .contains("parity@example.invalid"));
    let env = w.base_env();
    assert!(env.contains(&("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string())));
    let name = w.task_name("t");
    let hex = name.strip_prefix("t-").unwrap();
    assert!(
        hex.len() == 6
            && hex
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

#[test]
fn clean_teardown_ok_and_idempotent() {
    let mut w = world();
    assert_eq!(w.teardown(), Ok(()));
    assert_eq!(w.teardown(), Ok(()));
}

#[test]
fn leak_detected_and_killed() {
    let mut w = world();
    let script = r#"trap "" TERM; while :; do sleep 0.1; done"#;
    let mut child = w
        .command("sh")
        .arg("-c")
        .arg(script)
        .arg(w.root())
        .spawn()
        .unwrap();
    w.track_pid(child.id());
    std::thread::sleep(Duration::from_millis(200));
    let err = w.teardown().unwrap_err();
    assert!(err.starts_with("leaked processes: "), "{err}");
    assert!(err.contains(&child.id().to_string()), "{err}");
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert!(find_marked(&format!("PARITY_FAKES={}", w.fakes_dir().display())).is_empty());
}
