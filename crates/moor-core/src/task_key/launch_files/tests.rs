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

/// Runs the xcrun shim text against a fake real `xcrun` and a fake `xcodebuild` target,
/// each printing its name then one argument per line.
#[cfg(unix)]
fn run_xcrun_shim(tag: &str, args: &[&str]) -> String {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("moor-xcrun-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let l = layout(base.to_str().unwrap());
    let bin = l.bin_dir().unwrap();
    std::fs::create_dir_all(bin.as_str()).unwrap();
    let put = |path: &str, who: &str| {
        std::fs::write(
            path,
            format!("#!/bin/sh\necho {who}\nprintf '%s\\n' \"$@\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let real = format!("{}/real-xcrun", base.display());
    put(&real, "real");
    put(&format!("{}/xcodebuild", bin.as_str()), "shim");
    let shim = format!("{}/xcrun", base.display());
    std::fs::write(&shim, l.xcrun_shim_with(&real).unwrap()).unwrap();
    let out = Command::new("sh").arg(&shim).args(args).output().unwrap();
    ok(&out);
    let _ = std::fs::remove_dir_all(&base);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[cfg(unix)]
#[test]
fn xcrun_shim_passes_args_through_to_real_xcrun_intact() {
    assert_eq!(
        run_xcrun_shim("pass", &["simctl", "list", "a b"]),
        "real\nsimctl\nlist\na b\n"
    );
}

#[cfg(unix)]
#[test]
fn xcrun_shim_leaves_sdk_and_find_forms_to_real_xcrun() {
    assert_eq!(
        run_xcrun_shim("sdk", &["--sdk", "iphoneos", "xcodebuild", "-version"]),
        "real\n--sdk\niphoneos\nxcodebuild\n-version\n"
    );
    assert_eq!(
        run_xcrun_shim("find", &["-f", "xcodebuild"]),
        "real\n-f\nxcodebuild\n"
    );
}

#[cfg(unix)]
#[test]
fn xcrun_shim_sends_bare_xcodebuild_to_the_xcodebuild_shim() {
    assert_eq!(
        run_xcrun_shim("bare", &["xcodebuild", "-list", "x y"]),
        "shim\n-list\nx y\n"
    );
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

#[cfg(unix)]
fn sh_n(text: &str) {
    let out = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(text)
        .output()
        .unwrap();
    ok(&out);
}

#[cfg(unix)]
#[test]
fn shell_hostile_launch_root_keeps_scripts_and_shims_valid() {
    use crate::boxscript::quote;
    let base = std::env::temp_dir().join(format!("moor-shim-odd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let tools = base.join("tools");
    let dev = base.join("Developer");
    std::fs::create_dir_all(dev.join("usr/bin")).unwrap();
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(
        dev.join("usr/bin/xcodebuild"),
        "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n",
    )
    .unwrap();
    fake_xcode_select(&tools.join("xcode-select"), dev.to_str().unwrap());
    {
        use std::os::unix::fs::PermissionsExt;
        let real = dev.join("usr/bin/xcodebuild");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let root = format!("{}/we\"ird $HOME `id` x", base.display());
    let l = layout(&root);
    sh_n(&l.xcodebuild_shim_text().unwrap());
    sh_n(&l.xcrun_shim_text().unwrap());
    let script = l.prepare_script().unwrap();
    sh_n(&script);
    let path = format!("{}:/usr/bin:/bin", tools.display());
    ok(&run(&script, &path));

    let shim = quote(&format!("{}/xcodebuild", l.bin_dir().unwrap().as_str()));
    let out = run(&format!("{shim} -version"), &path);
    ok(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{FLAGS}-version\n")
    );

    let file = l.developer_dir_file().unwrap();
    std::fs::set_permissions(file.as_str(), {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(0o644)
    })
    .unwrap();
    std::fs::write(file.as_str(), "").unwrap();
    let out = run(&format!("{shim} -version"), &path);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(file.as_str()), "{err}");
    let _ = std::fs::remove_dir_all(&base);
}
