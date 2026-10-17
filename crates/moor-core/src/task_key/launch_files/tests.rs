//! Tests for the Mac Box launch files and prepare script.

use crate::name::{BoxPath, TaskName};
use crate::task_key::{MacLayout, TaskKey};
use std::path::Path;
use std::process::Command;

fn layout(root: &str) -> MacLayout {
    let key = TaskKey::new(&TaskName::parse("t1").unwrap());
    MacLayout::new(&BoxPath::restore(root).unwrap(), &key).unwrap()
}

#[test]
fn file_paths_are_under_launch() {
    let l = layout("/Users/Shared/moor-n");
    assert_eq!(
        l.xcconfig_file().unwrap().as_str(),
        "/Users/Shared/moor-n/t1/launch/moor.xcconfig"
    );
    assert_eq!(
        l.developer_dir_file().unwrap().as_str(),
        "/Users/Shared/moor-n/t1/launch/developer-dir"
    );
}

#[test]
fn xcconfig_text_is_exact() {
    let l = layout("/Users/Shared/moor-n");
    assert_eq!(
        l.xcconfig_text(),
        "HOME = /Users/Shared/moor-n/t1/home\n\
         OTHER_SWIFT_FLAGS = $(inherited) -disable-sandbox\n\
         ONLY_ACTIVE_ARCH = YES\n\
         CODE_SIGNING_ALLOWED = NO\n\
         CODE_SIGNING_REQUIRED = NO\n\
         CODE_SIGN_IDENTITY =\n"
    );
}

#[test]
fn script_has_quoted_paths_and_guards() {
    let l = layout("/Users/Shared/moor-n");
    let s = l.prepare_script().unwrap();
    assert!(s.starts_with("set -e\n"));
    for dir in [
        "home/Library/Caches",
        "home/Library/Preferences",
        "home/Library/Developer",
        "tmp",
        "cache",
        "sim",
        "launch",
    ] {
        assert!(
            s.contains(&format!("'/Users/Shared/moor-n/t1/{dir}'")),
            "{dir}"
        );
    }
    assert!(s.contains("chmod 700 '/Users/Shared/moor-n/t1/home' '/Users/Shared/moor-n/t1/tmp' '/Users/Shared/moor-n/t1/launch'"));
    assert!(s.contains("chmod 0444"));
    assert!(s.contains("mv -f \"$t\" '/Users/Shared/moor-n/t1/launch/moor.xcconfig'"));
    assert!(s.contains("if [ ! -s '/Users/Shared/moor-n/t1/launch/developer-dir' ]"));
    assert!(s.contains("xcode-select -p"));
    assert!(!s.contains("'/tmp"));
}

fn run(script: &str, path: &str) -> std::process::Output {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("PATH", path)
        .output()
        .unwrap()
}

#[cfg(unix)]
fn mode(p: &str) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
fn fake_xcode_select(fake: &Path, dir: &str) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::remove_file(fake);
    std::fs::write(fake, format!("#!/bin/sh\necho {dir}\n")).unwrap();
    std::fs::set_permissions(fake, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn ok(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn script_is_idempotent_and_reads_developer_dir_once() {
    let base = std::env::temp_dir().join(format!("moor-launch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("xcode-select");
    let a = "/Applications/Xcode-A.app/Contents/Developer";
    fake_xcode_select(&fake, a);
    let l = layout(base.join("work").to_str().unwrap());
    let script = l.prepare_script().unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let dev = l.developer_dir_file().unwrap();
    let xc = l.xcconfig_file().unwrap();

    ok(&run(&script, &path));
    assert_eq!(
        std::fs::read_to_string(dev.as_str()).unwrap(),
        format!("{a}\n")
    );
    assert_eq!(
        std::fs::read_to_string(xc.as_str()).unwrap(),
        l.xcconfig_text()
    );
    assert_eq!(mode(xc.as_str()), 0o444);
    assert_eq!(mode(dev.as_str()), 0o444);
    assert_eq!(mode(l.launch.as_str()), 0o700);

    fake_xcode_select(&fake, "/Applications/Xcode-B.app/Contents/Developer");
    ok(&run(&script, &path));
    assert_eq!(
        std::fs::read_to_string(dev.as_str()).unwrap(),
        format!("{a}\n")
    );
    assert_eq!(
        std::fs::read_to_string(xc.as_str()).unwrap(),
        l.xcconfig_text()
    );
    assert_eq!(
        std::fs::read_dir(l.launch.as_str()).unwrap().count(),
        2,
        "no temp files left"
    );
    let _ = std::fs::remove_dir_all(&base);
}
