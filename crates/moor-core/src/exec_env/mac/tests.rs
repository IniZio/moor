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
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: None,
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
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: None,
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
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: None,
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
    assert!(!pre.contains("CA_FILE_PATH"), "{pre}");
    assert!(pre.contains("export YARN_HTTP_PROXY="));
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

fn with_toolchains(
    env: &[(String, String)],
    bin: &[String],
) -> (Vec<(String, String)>, Vec<String>) {
    let (layout, key) = fixture();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
        toolchain_env: env,
        toolchain_bin: bin,
        sim_udid: None,
    };
    let b = BoxCtx {
        env: &[],
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(mac),
    };
    let got = build(&b, Purpose::Run).unwrap();
    (got.vars, got.prelude)
}

#[test]
fn toolchain_env_follows_the_task_vars_and_never_replaces_one() {
    let env = vec![
        (
            "MISE_DATA_DIR".to_owned(),
            "/Users/me/.local/share/mise".to_owned(),
        ),
        ("HOME".to_owned(), "/Users/me".to_owned()),
        ("GEM_HOME".to_owned(), "/elsewhere".to_owned()),
    ];
    let (vars, _) = with_toolchains(&env, &[]);
    assert_eq!(
        get(&vars, "MISE_DATA_DIR"),
        vec!["/Users/me/.local/share/mise".to_owned()]
    );
    assert_eq!(
        get(&vars, "HOME"),
        vec!["/Users/Shared/moor-n/t1/home".to_owned()]
    );
    assert_eq!(
        get(&vars, "GEM_HOME"),
        vec!["/Users/Shared/moor-n/t1/cache/gem".to_owned()]
    );
}

#[test]
fn toolchain_bins_precede_homebrew_in_order_and_are_quoted() {
    let bin = vec![
        "/Users/me/tool dir/bin".to_owned(),
        "/opt/two/bin".to_owned(),
    ];
    let (_, pre) = with_toolchains(&[], &bin);
    let script = format!("{}\necho \"$PATH\"", pre.join("\n"));
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let got = String::from_utf8(out.stdout).unwrap();
    let want = "/Users/Shared/moor-n/t1/launch/bin:/Users/me/tool dir/bin:/opt/two/bin:\
                /opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin";
    assert_eq!(got.trim(), want);
}

#[test]
fn empty_toolchains_leave_the_path_line_unchanged() {
    let (_, pre) = with_toolchains(&[], &[]);
    let line = pre.iter().find(|l| l.starts_with("PATH=")).unwrap();
    assert_eq!(
        line,
        "PATH='/Users/Shared/moor-n/t1/launch/bin':/opt/homebrew/bin:/opt/homebrew/sbin:\"$PATH\"; export PATH"
    );
}

#[test]
fn a_task_with_a_simulator_gets_its_device_and_set() {
    let (layout, key) = fixture();
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid: Some("0A1B2C3D-0000-1111-2222-333344445555"),
    };
    let v = vars(&mac).unwrap();
    assert_eq!(
        get(&v, "MOOR_SIM_UDID"),
        ["0A1B2C3D-0000-1111-2222-333344445555"]
    );
    assert_eq!(get(&v, "MOOR_SIM_SET"), ["/Users/Shared/moor-n/t1/sim"]);
}

#[test]
fn a_task_without_a_simulator_sets_neither_variable() {
    let (layout, key) = fixture();
    let tc = [("MOOR_SIM_UDID".to_owned(), "x".to_owned())];
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
        toolchain_env: &tc,
        toolchain_bin: &[],
        sim_udid: None,
    };
    let v = vars(&mac).unwrap();
    assert_eq!(get(&v, "MOOR_SIM_SET"), Vec::<String>::new());
    assert_eq!(get(&v, "MOOR_SIM_UDID"), ["x"]);
}

#[test]
fn a_toolchain_cannot_override_the_simulator_device() {
    let (layout, key) = fixture();
    let tc = [("MOOR_SIM_UDID".to_owned(), "x".to_owned())];
    let mac = MacTask {
        layout: &layout,
        key: &key,
        nonce: "abc",
        toolchain_env: &tc,
        toolchain_bin: &[],
        sim_udid: Some("real"),
    };
    assert_eq!(get(&vars(&mac).unwrap(), "MOOR_SIM_UDID"), ["real"]);
}
