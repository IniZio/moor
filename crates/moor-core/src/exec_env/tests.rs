use std::time::Duration;

use super::*;

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn env() -> Vec<(String, String)> {
    vec![("A".to_owned(), "a".to_owned())]
}

fn ctx<'a>(
    env: &'a [(String, String)],
    cred: Option<&'a BoxPath>,
    workdir: Option<&'a BoxPath>,
) -> BoxCtx<'a> {
    BoxCtx {
        env,
        proxy: cred.map(|cred| ProxyCtx {
            box_port: 4321,
            cred,
        }),
        origin: Some("https://github.com/o/n.git"),
        workdir,
        strip: &[],
    }
}

#[test]
fn every_purpose_shares_vars_prelude_and_cwd() {
    let (cred, wd, e) = (path("/cred"), path("/work"), env());
    let b = ctx(&e, Some(&cred), Some(&wd));
    let min = |m| Some(Deadline(Duration::from_mins(m)));
    let table = [
        (Purpose::Setup { lines: 2 }, min(60), false),
        (Purpose::Teardown, min(30), false),
        (
            Purpose::Run,
            Some(Deadline(Duration::from_hours(24))),
            false,
        ),
        (Purpose::Agent, None, true),
    ];
    let first = build(&b, table[0].0).unwrap();
    assert_eq!(first.vars, e);
    assert_eq!(first.cwd.as_ref(), Some(&wd));
    for (purpose, deadline, pty) in table {
        let got = build(&b, purpose).unwrap();
        assert_eq!(
            (got.vars.as_slice(), &got.prelude, &got.cwd),
            (first.vars.as_slice(), &first.prelude, &first.cwd),
            "{purpose:?}"
        );
        assert_eq!((got.deadline, got.pty), (deadline, pty), "{purpose:?}");
    }
}

#[test]
fn proxy_prelude_exports_from_the_staged_nonce() {
    let (cred, wd, e) = (path("/cred"), path("/work"), env());
    let got = build(&ctx(&e, Some(&cred), Some(&wd)), Purpose::Run).unwrap();
    let text = got.prelude.join("\n");
    assert!(text.contains("export HTTPS_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\""));
    assert!(text.contains("export SSL_CERT_FILE="));
    assert!(text.contains("export NODE_EXTRA_CA_CERTS='/cred/proxy-ca.pem'"));
    assert!(text.contains("/cred/proxy-nonce"));
    assert!(text.contains("GIT_CONFIG_VALUE_0='https://github.com/o/n.git'"));
    assert!(got.prelude.last().unwrap().starts_with("unset moor_nonce"));
    for name in PASS_VARS {
        assert!(text.contains(&format!("export {name}=")), "{name}");
    }
}

#[test]
fn no_proxy_means_no_proxy_exports() {
    let (wd, e) = (path("/work"), env());
    let got = build(&ctx(&e, None, Some(&wd)), Purpose::Run).unwrap();
    assert!(got.prelude.is_empty());
    let argv = vec!["ls".to_owned(), "-l".to_owned()];
    assert_eq!(wrap_argv(&got, argv.clone(), None), argv);
}

#[test]
fn workdir_none_means_cwd_none() {
    let e = env();
    let b = ctx(&e, None, None);
    assert_eq!(build(&b, Purpose::Run).unwrap().cwd, None);
}

#[test]
fn a_bad_env_name_is_an_error() {
    let (wd, cred) = (path("/work"), path("/cred"));
    let bad = vec![("A B".to_owned(), "x".to_owned())];
    assert!(build(&ctx(&bad, Some(&cred), Some(&wd)), Purpose::Agent).is_err());
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn wrap_argv_runs_the_prelude_first() {
    let (cred, wd, e) = (path("/cred"), path("/w"), env());
    let mut got = build(&ctx(&e, Some(&cred), Some(&wd)), Purpose::Run).unwrap();
    let argv = strs(&["ls", "-l"]);
    let script = format!("{}\nexec \"$@\"", got.prelude.join("\n"));
    assert_eq!(
        wrap_argv(&got, argv.clone(), None),
        [strs(&["sh", "-c", &script, "sh"]), argv.clone()].concat()
    );
    got.prelude.clear();
    assert_eq!(wrap_argv(&got, argv.clone(), None), argv);
}

#[test]
fn cleanup_traps_the_removal_and_does_not_exec() {
    let (cred, wd, e) = (path("/cred"), path("/w"), env());
    let got = build(&ctx(&e, Some(&cred), Some(&wd)), Purpose::Run).unwrap();
    let dir = path("/it's/cred");
    let wrapped = wrap_argv(&got, strs(&["ls"]), Some(&dir));
    assert_eq!(wrapped.len(), 5, "{wrapped:?}");
    let script = wrapped.get(2).unwrap();
    assert_eq!(wrapped.get(..2).unwrap(), strs(&["sh", "-c"]));
    assert_eq!(wrapped.get(3..).unwrap(), strs(&["sh", "ls"]));
    let trap = format!("trap {} EXIT\n", quote("rm -rf '/it'\\''s/cred'"));
    assert!(script.starts_with(&trap), "{script}");
    assert!(script.ends_with("\n\"$@\""));
    assert!(!script.contains("exec"));
    assert!(script.contains(got.prelude.first().unwrap().as_str()));
    // Cleanup alone still wraps when the prelude is empty.
    let bare = build(&ctx(&e, None, None), Purpose::Run).unwrap();
    let w = wrap_argv(&bare, strs(&["ls"]), Some(&dir));
    let expect = format!("{trap}\"$@\"");
    assert_eq!(w, strs(&["sh", "-c", &expect, "sh", "ls"]));
}

#[test]
fn cleanup_removes_the_directory_and_keeps_the_exit_status() {
    let e = env();
    let bare = build(&ctx(&e, None, None), Purpose::Run).unwrap();
    for (cmd, code) in [("exit 0", 0), ("exit 3", 3)] {
        let name = format!("moor-exec-env-{}-{code}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proxy-nonce"), "x").unwrap();
        let bp = BoxPath::restore(dir.to_str().unwrap()).unwrap();
        let w = wrap_argv(&bare, strs(&["sh", "-c", cmd]), Some(&bp));
        let (prog, args) = w.split_first().unwrap();
        let status = std::process::Command::new(prog)
            .args(args)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(code), "{cmd}");
        assert!(!dir.exists(), "{cmd}");
    }
}

#[test]
fn setup_deadline_scales_with_lines() {
    let (wd, e) = (path("/w"), env());
    let b = ctx(&e, None, Some(&wd));
    for (lines, mins) in [(0, 0), (1, 30), (3, 90)] {
        let got = build(&b, Purpose::Setup { lines }).unwrap();
        assert_eq!(got.deadline, Some(Deadline(Duration::from_mins(mins))));
    }
}

#[test]
fn merged_env_lets_rules_win() {
    let repo = vec![
        ("A".to_owned(), "1".to_owned()),
        ("B".to_owned(), "2".to_owned()),
    ];
    let rules = vec![("A".to_owned(), "9".to_owned())];
    assert_eq!(
        merged_env(repo, rules),
        [
            ("B".to_owned(), "2".to_owned()),
            ("A".to_owned(), "9".to_owned())
        ]
    );
}

#[test]
fn github_origins_map_to_one_push_url() {
    let want = Some("https://github.com/o/n.git".to_owned());
    for origin in [
        "https://github.com/o/n",
        "https://github.com/o/n.git",
        "https://github.com/o/n/",
        "ssh://git@github.com/o/n.git",
        "ssh://git@github.com/o/n",
        "git@github.com:o/n.git",
        "git@github.com:o/n",
    ] {
        assert_eq!(push_url(origin), want, "{origin}");
    }
}

#[test]
fn other_origins_are_not_github() {
    for origin in [
        "https://gitlab.com/o/n.git",
        "git@gitlab.com:o/n.git",
        "ssh://git@example.com/o/n.git",
        "/srv/git/o/n.git",
        "https://github.com/o",
        "https://github.com/o/",
        "https://github.com/o/n/extra",
        "https://github.com/o/n/extra.git",
        "",
    ] {
        assert_eq!(push_url(origin), None, "{origin}");
    }
}

#[test]
fn strip_is_the_same_for_every_purpose_and_leaves_vars() {
    let (cred, wd) = (path("/cred"), path("/work"));
    let e = vec![
        ("A".to_owned(), "a".to_owned()),
        ("ANTHROPIC_API_KEY".to_owned(), "k".to_owned()),
    ];
    let strip = strs(&["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]);
    let mut b = ctx(&e, Some(&cred), Some(&wd));
    b.strip = &strip;
    let first = build(&b, Purpose::Run).unwrap();
    assert_eq!(first.unset, strs(&["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]));
    assert_eq!(first.vars, [("A".to_owned(), "a".to_owned())]);
    for purpose in [
        Purpose::Setup { lines: 1 },
        Purpose::Teardown,
        Purpose::Agent,
    ] {
        let got = build(&b, purpose).unwrap();
        assert_eq!((&got.unset, &got.vars), (&first.unset, &first.vars));
    }
    b.strip = &[];
    assert!(build(&b, Purpose::Run).unwrap().unset.is_empty());
}

#[test]
fn a_bad_strip_name_is_an_error() {
    let (wd, e) = (path("/work"), env());
    let strip = strs(&["A B"]);
    let mut b = ctx(&e, None, Some(&wd));
    b.strip = &strip;
    assert!(build(&b, Purpose::Run).is_err());
}

#[test]
fn prelude_exports_yarn_after_the_proxy_and_before_no_proxy() {
    let (cred, wd, e) = (path("/cred"), path("/work"), env());
    let got = build(&ctx(&e, Some(&cred), Some(&wd)), Purpose::Agent).unwrap();
    let at = |needle: &str| got.prelude.iter().position(|l| l.contains(needle)).unwrap();
    let yarn = [
        "export YARN_HTTPS_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export YARN_HTTP_PROXY=\"http://moor:${moor_nonce}@127.0.0.1:4321\"",
        "export YARN_CA_FILE_PATH=\"$moor_bundle\"",
        "export YARN_CACHE_FOLDER=\"$HOME/.cache/yarn\"",
        "export YARN_GLOBAL_FOLDER=\"$HOME/.cache/yarn/berry\"",
        "export npm_config_cache=\"$HOME/.cache/npm\"",
    ];
    let first = at("export http_proxy=") + 1;
    assert_eq!(
        got.prelude.get(first..first + 6),
        Some(strs(&yarn).as_slice())
    );
    assert_eq!(first + 6, at("export NO_PROXY="));
}

#[test]
fn stage_steps_put_the_nonce_private_and_the_ca_public() {
    let cred = path("/cred");
    let nonce = Secret::new(b"secret-nonce".to_vec());
    let [(n, nmsg), (c, cmsg)] = stage_steps(&cred, &nonce, "PEM").unwrap();
    assert_eq!(
        n,
        BoxStep::Put {
            path: path("/cred/proxy-nonce"),
            bytes: b"secret-nonce".to_vec(),
            mode: 0o600
        }
    );
    assert_eq!(
        c,
        BoxStep::Put {
            path: path("/cred/proxy-ca.pem"),
            bytes: b"PEM".to_vec(),
            mode: 0o644
        }
    );
    assert_eq!(nmsg, "could not stage the proxy nonce");
    assert_eq!(cmsg, "could not stage the proxy CA");
    assert!(stage_steps(&cred, &Secret::new(Vec::new()), "PEM").is_err());
}

#[test]
fn wrap_argv_unsets_first_even_without_a_prelude() {
    let (wd, e) = (path("/w"), env());
    let strip = strs(&["X", "Y"]);
    let mut b = ctx(&e, None, Some(&wd));
    b.strip = &strip;
    let got = build(&b, Purpose::Run).unwrap();
    assert_eq!(
        wrap_argv(&got, strs(&["ls"]), None),
        strs(&["sh", "-c", "unset X Y\nexec \"$@\"", "sh", "ls"])
    );
    let (cred, e2) = (path("/cred"), env());
    let mut p = ctx(&e2, Some(&cred), Some(&wd));
    p.strip = &strip;
    let with = build(&p, Purpose::Run).unwrap();
    let w = wrap_argv(&with, strs(&["ls"]), None);
    assert!(w.get(2).unwrap().starts_with("unset X Y\n"), "{w:?}");
    assert!(w.get(2).unwrap().contains("\nunset GH_TOKEN"));
}

#[test]
fn exec_purposes_reexport_the_token_after_the_unset_and_the_agent_does_not() {
    let (cred, wd) = (path("/cred"), path("/work"));
    let e = vec![("GH_TOKEN".to_owned(), "moor-proxy".to_owned())];
    let b = ctx(&e, Some(&cred), Some(&wd));
    for purpose in [Purpose::Run, Purpose::Setup { lines: 1 }, Purpose::Teardown] {
        let got = build(&b, purpose).unwrap();
        let at = |needle: &str| got.prelude.iter().position(|l| l == needle).unwrap();
        assert!(
            at("export GH_TOKEN='moor-proxy'") > at("unset GH_TOKEN GITHUB_TOKEN"),
            "{purpose:?}"
        );
    }
    let agent = build(&b, Purpose::Agent).unwrap();
    assert!(!agent.prelude.join("\n").contains("export GH_TOKEN="));
}

#[test]
fn a_stripped_token_is_not_reexported() {
    let (cred, wd) = (path("/cred"), path("/work"));
    let e = vec![("GH_TOKEN".to_owned(), "moor-proxy".to_owned())];
    let strip = ["GH_TOKEN".to_owned()];
    let b = BoxCtx {
        strip: &strip,
        ..ctx(&e, Some(&cred), Some(&wd))
    };
    let got = build(&b, Purpose::Run).unwrap();
    assert!(!got.prelude.join("\n").contains("export GH_TOKEN="));
}
