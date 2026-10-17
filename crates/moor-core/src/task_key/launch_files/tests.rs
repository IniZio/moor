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
        3,
        "no temp files left"
    );
    assert_eq!(
        std::fs::read_dir(l.bin_dir().unwrap().as_str())
            .unwrap()
            .count(),
        2,
        "no temp files left in bin"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn bin_dir_is_under_launch() {
    let l = layout("/Users/Shared/moor-n");
    assert_eq!(
        l.bin_dir().unwrap().as_str(),
        "/Users/Shared/moor-n/t1/launch/bin"
    );
}

#[test]
fn script_writes_both_shims_read_only() {
    let l = layout("/Users/Shared/moor-n");
    let s = l.prepare_script().unwrap();
    assert!(s.contains("'/Users/Shared/moor-n/t1/launch/bin'"));
    assert!(s.contains("mktemp '/Users/Shared/moor-n/t1/launch/bin'/.xcodebuild.XXXXXX"));
    assert!(s.contains("mv -f \"$t\" '/Users/Shared/moor-n/t1/launch/bin/xcodebuild'"));
    assert!(s.contains("mv -f \"$t\" '/Users/Shared/moor-n/t1/launch/bin/xcrun'"));
    assert_eq!(s.matches("chmod 0555 \"$t\"").count(), 2);
}

#[cfg(unix)]
struct Prepared {
    base: std::path::PathBuf,
    l: MacLayout,
    script: String,
    path: String,
}

/// Runs the prepare script with a fake `xcode-select` whose Xcode has a
/// fake `xcodebuild` that prints its arguments one per line.
#[cfg(unix)]
fn prepared(tag: &str) -> Prepared {
    let base = std::env::temp_dir().join(format!("moor-shim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let tools = base.join("tools");
    let dev = base.join("Developer");
    std::fs::create_dir_all(dev.join("usr/bin")).unwrap();
    std::fs::create_dir_all(&tools).unwrap();
    fake_xcode_select(&dev.join("usr/bin/xcodebuild"), "");
    std::fs::write(
        dev.join("usr/bin/xcodebuild"),
        "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n",
    )
    .unwrap();
    fake_xcode_select(&tools.join("xcode-select"), dev.to_str().unwrap());
    let l = layout(base.join("work").to_str().unwrap());
    let script = l.prepare_script().unwrap();
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        l.bin_dir().unwrap().as_str(),
        tools.display()
    );
    ok(&run(&script, &path));
    Prepared {
        base,
        l,
        script,
        path,
    }
}

const FLAGS: &str = "-IDEPackageSupportDisableManifestSandbox=YES\n\
                     -IDEPackageSupportDisablePluginExecutionSandbox=YES\n";

#[cfg(unix)]
#[test]
fn xcodebuild_shim_adds_flags_and_reaches_the_real_binary() {
    let p = prepared("xb");
    let shim = format!("{}/xcodebuild", p.l.bin_dir().unwrap().as_str());
    let out = run(&format!("'{shim}' -version"), &p.path);
    ok(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{FLAGS}-version\n")
    );
    let _ = std::fs::remove_dir_all(&p.base);
}

#[cfg(unix)]
#[test]
fn xcrun_shim_routes_xcodebuild_through_the_flags() {
    let p = prepared("xr");
    let shim = format!("{}/xcrun", p.l.bin_dir().unwrap().as_str());
    let out = run(&format!("'{shim}' xcodebuild -list"), &p.path);
    ok(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{FLAGS}-list\n")
    );
    let _ = std::fs::remove_dir_all(&p.base);
}

#[cfg(unix)]
#[test]
fn rerunning_the_script_keeps_shims_read_only_and_unchanged() {
    let p = prepared("re");
    let bin = p.l.bin_dir().unwrap();
    let files = [
        format!("{}/xcodebuild", bin.as_str()),
        format!("{}/xcrun", bin.as_str()),
    ];
    let before = files.clone().map(|f| std::fs::read_to_string(f).unwrap());
    ok(&run(&p.script, &p.path));
    for (f, b) in files.iter().zip(before) {
        assert_eq!(mode(f), 0o555);
        assert_eq!(std::fs::read_to_string(f).unwrap(), b);
    }
    let _ = std::fs::remove_dir_all(&p.base);
}
