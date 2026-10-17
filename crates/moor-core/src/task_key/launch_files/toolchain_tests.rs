//! Tests for the toolchain link script.

use crate::name::{BoxPath, TaskName};
use crate::task_key::{MacLayout, TaskKey};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn layout(root: &str) -> MacLayout {
    let key = TaskKey::new(&TaskName::parse("t1").unwrap());
    MacLayout::new(&BoxPath::restore(root).unwrap(), &key).unwrap()
}

fn tc(name: &str, root: &str) -> (String, String) {
    (name.to_owned(), root.to_owned())
}

fn sandbox(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("moor-tc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base.canonicalize().unwrap()
}

fn run(script: &str, home: &Path) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn empty_list_gives_empty_script() {
    assert_eq!(layout("/w").toolchain_script(&[]).unwrap(), "");
}

#[test]
fn script_text_for_a_root_under_home_and_one_outside() {
    let l = layout("/Users/Shared/moor-n");
    let s = l
        .toolchain_script(&[tc("mise", "/Users/me/.mise"), tc("sdk", "/opt/sdk")])
        .unwrap();
    let h = |rel: &str| format!("\"$HOME\"'{rel}'");
    let w = "'/Users/Shared/moor-n'";
    let inside = [
        h("/Library") + "/*",
        h("/.config/gh"),
        h("/.config/gh") + "/*",
        h("/.docker"),
        h("/.docker") + "/*",
        h("/.ssh"),
        h("/.ssh") + "/*",
        w.to_owned(),
        format!("{w}/*"),
    ]
    .join("|");
    let mut contains = String::new();
    for s in [
        "\"$HOME\"".to_owned(),
        h("/.config/gh"),
        h("/.docker"),
        h("/.ssh"),
        w.to_owned(),
    ] {
        writeln!(contains, "case {s} in (\"$r\"|\"${{r%/}}\"/*) k=0;; esac").unwrap();
    }
    let one = |root: &str, name: &str| {
        format!(
            "if ! r=$(cd '{root}' 2>/dev/null && pwd -P); then printf '%s\\n' 'host toolchain \"{name}\": root {root} is not a directory on the Box' >&2; exit 1; fi\n\
             p='{root}'\n\
             k=1\n\
             case $r in ({inside}) k=0;; esac\n\
             {contains}\
             if [ \"$k\" = 1 ]; then\n\
             case $p in\n\
             (\"$HOME\"/*)\n\
             l='/Users/Shared/moor-n/t1/home'/${{p#\"$HOME\"/}}\n\
             mkdir -p \"${{l%/*}}\"\n\
             if [ -L \"$l\" ] || [ ! -e \"$l\" ]; then ln -sfn '{root}' \"$l\"; else printf '%s %s\\n' 'host toolchain \"{name}\": cannot link the root because this path exists and is not a link:' \"$l\" >&2; exit 1; fi\n\
             ;;\n\
             esac\n\
             fi\n\
             printf '%s\\n' \"$r\"\n"
        )
    };
    assert_eq!(
        s,
        format!(
            "{}{}",
            one("/Users/me/.mise", "mise"),
            one("/opt/sdk", "sdk")
        )
    );
}

#[test]
fn newline_or_relative_root_is_refused() {
    let l = layout("/w");
    assert!(l.toolchain_script(&[tc("a", "/x\n/y")]).is_err());
    assert!(l.toolchain_script(&[tc("a\nb", "/x")]).is_err());
    assert!(l.toolchain_script(&[tc("a", "rel/x")]).is_err());
}

#[test]
fn links_root_prints_realpath_and_is_idempotent() {
    let base = sandbox("link");
    let home = base.join("login");
    let real = base.join("real-tools");
    std::fs::create_dir_all(home.join(".local/share")).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    let root = home.join(".local/share/mise");
    std::os::unix::fs::symlink(&real, &root).unwrap();
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();

    let l = layout(base.join("work").to_str().unwrap());
    let script = l
        .toolchain_script(&[
            tc("mise", root.to_str().unwrap()),
            tc("sdk", outside.to_str().unwrap()),
        ])
        .unwrap();
    let expect = format!("{}\n{}\n", real.display(), outside.display());
    for _ in 0..2 {
        let out = run(&script, &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), expect);
        let link = Path::new(l.home.as_str()).join(".local/share/mise");
        assert_eq!(std::fs::read_link(&link).unwrap(), root);
    }
    assert!(!Path::new(l.home.as_str()).join("outside").exists());
}

#[test]
fn existing_real_directory_is_refused() {
    let base = sandbox("clash");
    let home = base.join("login");
    let root = home.join("tools");
    std::fs::create_dir_all(&root).unwrap();
    let l = layout(base.join("work").to_str().unwrap());
    let clash = Path::new(l.home.as_str()).join("tools");
    std::fs::create_dir_all(&clash).unwrap();
    let script = l
        .toolchain_script(&[tc("t", root.to_str().unwrap())])
        .unwrap();
    let out = run(&script, &home);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("exists and is not a link"));
    assert!(clash.is_dir() && !clash.is_symlink());
}

#[test]
fn missing_root_exits_one_with_message() {
    let base = sandbox("missing");
    let l = layout(base.join("work").to_str().unwrap());
    let root = base.join("nope");
    let script = l
        .toolchain_script(&[tc("flutter", root.to_str().unwrap())])
        .unwrap();
    let out = run(&script, &base);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "host toolchain \"flutter\": root {} is not a directory on the Box\n",
            root.display()
        )
    );
    assert!(out.stdout.is_empty());
}

fn refused_case(tag: &str, root: impl Fn(&Path, &Path) -> PathBuf) {
    let base = sandbox(tag);
    let home = base.join("login");
    let work = home.join("work");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(home.join("Library/x")).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let root = root(&home, &work);
    let l = layout(work.join("moor-n").to_str().unwrap());
    let script = l
        .toolchain_script(&[tc("t", root.to_str().unwrap())])
        .unwrap();
    let out = run(&script, &home);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n", root.canonicalize().unwrap().display())
    );
    assert!(!Path::new(l.home.as_str()).exists());
}

#[test]
fn root_symlinked_to_ssh_is_not_linked() {
    refused_case("ssh", |home, _| {
        let root = home.join("tools");
        std::os::unix::fs::symlink(home.join(".ssh"), &root).unwrap();
        root
    });
}

#[test]
fn root_that_is_home_is_not_linked() {
    refused_case("home", |home, _| home.to_owned());
}

#[test]
fn root_inside_library_is_not_linked() {
    refused_case("lib", |home, _| home.join("Library/x"));
}

#[test]
fn root_above_home_is_not_linked() {
    refused_case("above", |home, _| home.parent().unwrap().to_owned());
}

#[test]
fn root_under_work_root_is_not_linked() {
    refused_case("workroot", |_, work| {
        let r = work.join("moor-n/t1/work");
        std::fs::create_dir_all(&r).unwrap();
        r
    });
}

#[test]
fn root_under_home_is_linked() {
    let base = sandbox("plain");
    let home = base.join("login");
    let root = home.join("tools");
    std::fs::create_dir_all(&root).unwrap();
    let l = layout(base.join("work").to_str().unwrap());
    let script = l
        .toolchain_script(&[tc("t", root.to_str().unwrap())])
        .unwrap();
    assert!(run(&script, &home).status.success());
    let link = Path::new(l.home.as_str()).join("tools");
    assert_eq!(std::fs::read_link(&link).unwrap(), root);
}

#[test]
fn root_holding_a_credential_dir_is_not_linked() {
    refused_case("holds-cred", |home, _| {
        std::fs::create_dir_all(home.join(".config/gh")).unwrap();
        home.join(".config")
    });
}
