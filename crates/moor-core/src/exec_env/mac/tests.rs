//! Tests for the per-Task Mac environment.

use super::*;
use crate::exec_env::{build, BoxCtx, ProxyCtx, Purpose};
use crate::name::TaskName;

fn bp(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn fixture() -> (MacLayout, TaskKey) {
    let key = TaskKey::new(&TaskName::parse("t1").unwrap());
    (
        MacLayout::new(&bp("/Users/Shared/moor-n"), &key).unwrap(),
        key,
    )
}

fn get(vars: &[(String, String)], name: &str) -> Vec<String> {
    vars.iter()
        .filter(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .collect()
}

#[test]
fn every_purpose_gets_the_task_vars() {
    let (layout, key) = fixture();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
    };
    let repo = vec![
        ("HOME".to_owned(), "/root".to_owned()),
        ("A".to_owned(), "a".to_owned()),
    ];
    let b = BoxCtx {
        env: &repo,
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(mac),
    };
    let r = "/Users/Shared/moor-n/t1";
    let want = [
        ("HOME", "home"),
        ("CFFIXED_USER_HOME", "home"),
        ("TMPDIR", "tmp"),
        ("XCODE_XCCONFIG_FILE", "launch/moor.xcconfig"),
        ("YARN_CACHE_FOLDER", "cache/yarn"),
        ("CP_CACHE_DIR", "cache/cocoapods/cache"),
        ("GEM_HOME", "cache/gem"),
        ("GEM_SPEC_CACHE", "cache/gem/specs"),
        ("PUB_CACHE", "cache/pub"),
        ("XDG_CONFIG_HOME", "home/.config"),
        ("XDG_DATA_HOME", "home/.local/share"),
    ];
    for purpose in [
        Purpose::Setup { lines: 1 },
        Purpose::Teardown,
        Purpose::Run,
        Purpose::Agent,
    ] {
        let got = build(&b, purpose).unwrap();
        for (name, rel) in want {
            assert_eq!(get(&got.vars, name), vec![format!("{r}/{rel}")], "{name}");
        }
        assert!(
            get(&got.vars, "GEM_PATH").is_empty(),
            "GEM_PATH stays unset so Ruby keeps its own gem directories"
        );
        assert_eq!(get(&got.vars, "MOOR_BOX_TAG"), vec!["t1-abc".to_owned()]);
        assert_eq!(get(&got.vars, "A"), vec!["a".to_owned()]);
        assert_prelude(&got.prelude, r);
        assert!(get(&got.vars, "PATH").is_empty(), "PATH stays out of vars");
    }
}

fn assert_prelude(prelude: &[String], r: &str) {
    let pre = prelude.join("\n");
    assert!(pre.contains(&format!("{r}/launch/developer-dir")), "{pre}");
    assert!(pre.contains("export DEVELOPER_DIR"), "{pre}");
    let line = prelude
        .iter()
        .find(|l| l.starts_with("PATH="))
        .expect("PATH line in prelude");
    let bin = line.find(&format!("{r}/launch/bin")).expect("shims dir");
    let brew = line.find("/opt/homebrew/bin").expect("homebrew");
    assert!(bin < brew, "{line}");
    assert!(line.ends_with("; export PATH"), "{line}");
}

#[test]
fn the_prelude_puts_shims_and_homebrew_first_on_the_inherited_path() {
    let (layout, key) = fixture();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
    };
    let b = BoxCtx {
        env: &[],
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(mac),
    };
    let pre = build(&b, Purpose::Run).unwrap().prelude;
    let script = format!("{}\necho \"$PATH\"", pre.join("\n"));
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let got = String::from_utf8(out.stdout).unwrap();
    let got = got.trim();
    let head = "/Users/Shared/moor-n/t1/launch/bin:/opt/homebrew/bin:/opt/homebrew/sbin:";
    assert!(got.starts_with(head), "{got}");
    assert!(got.ends_with("/usr/bin:/bin"), "{got}");
}

#[test]
fn proxy_cache_lines_do_not_override_task_caches() {
    let (layout, key) = fixture();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
    };
    let cred = bp("/cred");
    let b = BoxCtx {
        env: &[],
        proxy: Some(ProxyCtx {
            box_port: 4321,
            cred: &cred,
        }),
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(mac),
    };
    let pre = build(&b, Purpose::Run).unwrap().prelude.join("\n");
    assert!(!pre.contains("$HOME/.cache"), "{pre}");
    assert!(pre.contains("export YARN_HTTPS_PROXY="));
    assert!(pre.contains("export YARN_HTTPS_CA_FILE_PATH="));
}

#[test]
fn without_mac_there_is_no_developer_dir_or_path_line() {
    let b = BoxCtx {
        env: &[],
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: None,
    };
    assert!(build(&b, Purpose::Run).unwrap().prelude.is_empty());
}
