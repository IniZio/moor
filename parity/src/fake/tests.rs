use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn ctx<'a>(argv: &'a [String]) -> TemplateCtx<'a> {
    TemplateCtx {
        argv,
        n: 3,
        box_root: Path::new("/w/box"),
        root: Path::new("/w"),
        cwd: Path::new("/w/cwd"),
    }
}

#[test]
fn render_json_braces_untouched() {
    let argv = s(&["x"]);
    let t = r#"{"a":{"b":1}} {"result":{"n":"{n}"}}"#;
    assert_eq!(
        render(t, &ctx(&argv)),
        r#"{"a":{"b":1}} {"result":{"n":"3"}}"#
    );
    assert_eq!(render(r#"{"a":{"b":1}}"#, &ctx(&argv)), r#"{"a":{"b":1}}"#);
    assert_eq!(render("{unknown} {", &ctx(&argv)), "{unknown} {");
}

#[test]
fn render_each_placeholder() {
    let argv = s(&["run", "--slug", "foo", "--id=7", "--", "echo", "it's"]);
    let c = ctx(&argv);
    assert_eq!(render("{argv[0]}/{argv[2]}/{argv[99]}", &c), "run/foo/");
    assert_eq!(
        render("{flag:--slug}|{flag:--id}|{flag:--nope}", &c),
        "foo|7|"
    );
    assert_eq!(render("{n} {box} {root} {cwd}", &c), "3 /w/box /w /w/cwd");
    assert_eq!(render("{last}", &c), "it's");
    assert_eq!(render("{post}", &c), "'echo' 'it'\\''s'");
    assert_eq!(render("[{post}]", &ctx(&s(&["a"]))), "[]");
}

#[test]
fn shell_quote_escapes() {
    assert_eq!(shell_quote("a b"), "'a b'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
}

#[test]
fn match_semantics() {
    let none = |_: &str| false;
    let has = |v: &str| v == "FOO";
    let argv = s(&["run", "--env", "MOOR_WORKDIR=1"]);
    assert!(Match::default().matches(&argv, &none));
    assert!(Rule::on(&["run"]).matcher.matches(&argv, &none));
    assert!(!Rule::on(&["list"]).matcher.matches(&argv, &none));
    assert!(!Rule::on(&["run", "a", "b", "c"])
        .matcher
        .matches(&argv, &none));
    assert!(Rule::on(&["run"])
        .contains("WORKDIR")
        .matcher
        .matches(&argv, &none));
    assert!(!Rule::on(&["run"])
        .contains("WORKDIR")
        .contains("zzz")
        .matcher
        .matches(&argv, &none));
    assert!(Rule::any().env_has("FOO").matcher.matches(&argv, &has));
    assert!(!Rule::any().env_has("FOO").matcher.matches(&argv, &none));
}

#[test]
fn call_flag() {
    let mut c = Call {
        seq: 1,
        tool: "t".into(),
        argv: s(&["warmup", "--slug", "abc", "--x=1"]),
        cwd: String::new(),
        pid: 1,
        ppid: 1,
        stdin: String::new(),
        env: BTreeMap::new(),
        env_digests: BTreeMap::new(),
        rule: None,
        pgid: 0,
        starttime: 0,
        env_contains_secret: false,
    };
    assert_eq!(c.flag("--slug").as_deref(), Some("abc"));
    assert_eq!(c.flag("--x").as_deref(), Some("1"));
    assert_eq!(c.flag("--y"), None);
    c.argv = s(&["--slug"]);
    assert_eq!(c.flag("--slug"), None);
}

#[test]
fn append_prepend_order_and_ids() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert!(read_rules(d, "t").is_empty());
    append_rules(d, "t", vec![Rule::on(&["a"]), Rule::on(&["b"])]);
    prepend_rules(d, "t", vec![Rule::on(&["c"]).stdout("x")]);
    let mut keep = Rule::on(&["d"]);
    keep.id = "mine".into();
    append_rules(d, "t", vec![keep]);
    let rules = read_rules(d, "t");
    let order: Vec<_> = rules
        .iter()
        .map(|r| r.matcher.argv_prefix.clone().unwrap()[0].clone())
        .collect();
    assert_eq!(order, ["c", "a", "b", "d"]);
    let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["r3", "r1", "r2", "mine"]);
    assert_eq!(rules[0].reply.stdout, "x");
}

#[test]
fn read_secrets_skips_blank_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let p = secrets_path(dir.path());
    assert!(read_secrets(&p).is_empty());
    fs::write(&p, "one\n\ntwo\n\n").unwrap();
    assert_eq!(read_secrets(&p), ["one", "two"]);
}

#[test]
fn seq_and_calls() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let _g = lock(d).unwrap();
    assert_eq!(next_seq_locked(d), 1);
    assert_eq!(next_seq_locked(d), 2);
    let mk = |seq, tool: &str| Call {
        seq,
        tool: tool.into(),
        argv: vec![],
        cwd: String::new(),
        pid: 0,
        ppid: 0,
        stdin: String::new(),
        env: BTreeMap::new(),
        env_digests: BTreeMap::new(),
        rule: None,
        pgid: 0,
        starttime: 0,
        env_contains_secret: false,
    };
    let line = |c: &Call| format!("{}\n", serde_json::to_string(c).unwrap());
    fs::write(calls_path(d, "a"), line(&mk(2, "a"))).unwrap();
    fs::write(calls_path(d, "b"), line(&mk(1, "b")) + &line(&mk(3, "b"))).unwrap();
    let all: Vec<u64> = all_calls(d).iter().map(|c| c.seq).collect();
    assert_eq!(all, [1, 2, 3]);
    assert_eq!(read_calls(d, "b").len(), 2);
    assert!(read_calls(d, "zz").is_empty());
}
