use super::*;

fn m(s: &str) -> Vec<String> {
    split(s)
}

fn rule() -> Rule {
    Rule {
        krate: "fake".into(),
        module: m("domain"),
        allowed: vec![m("util")],
    }
}

fn bad(src: &str, file_mod: &str) -> Vec<Vec<String>> {
    let all = refs(src, &m(file_mod)).unwrap();
    violations(&all, &rule())
        .into_iter()
        .map(|r| r.to.clone())
        .collect()
}

#[test]
fn plain_use_is_flagged() {
    assert_eq!(
        bad("use crate::adapter::Thing;", "domain"),
        vec![m("adapter::Thing")]
    );
}

#[test]
fn allowed_and_own_tree_pass() {
    assert!(bad("use crate::util::H;\nuse crate::domain::Other;", "domain").is_empty());
}

#[test]
fn brace_group_flags_each_member() {
    let got = bad("use crate::{adapter::A, util::B, other::C};", "domain");
    assert_eq!(got, vec![m("adapter::A"), m("other::C")]);
}

#[test]
fn multi_line_braces() {
    let got = bad(
        "use crate::{\n    util::B,\n    adapter::{\n        A,\n        B,\n    },\n};",
        "domain",
    );
    assert_eq!(got, vec![m("adapter::A"), m("adapter::B")]);
    let all = refs(
        "use crate::{\n    util::B,\n    adapter::{\n        A,\n    },\n};",
        &m("domain"),
    )
    .unwrap();
    assert_eq!(all[1].line, 4);
}

#[test]
fn super_and_self_resolve_against_file_module() {
    assert_eq!(
        bad("use super::adapter::X;", "domain"),
        vec![m("adapter::X")]
    );
    assert!(bad("use super::adapter::X;", "domain::inner").is_empty());
    assert!(bad("use super::util::X;", "domain").is_empty());
    assert!(bad("use self::sub::X;", "domain").is_empty());
    assert_eq!(
        bad("use super::super::adapter::X;", "domain::a"),
        vec![m("adapter::X")]
    );
}

#[test]
fn glob_is_flagged() {
    assert_eq!(bad("use crate::adapter::*;", "domain"), vec![m("adapter")]);
}

#[test]
fn rename_is_flagged() {
    assert_eq!(
        bad("use crate::adapter::Thing as Other;", "domain"),
        vec![m("adapter::Thing")]
    );
}

#[test]
fn inline_path_in_type_and_expr() {
    let src = "fn f(x: crate::adapter::T) -> u32 { crate::adapter::g(x) }";
    assert_eq!(bad(src, "domain"), vec![m("adapter::T"), m("adapter::g")]);
}

#[test]
fn comments_and_strings_are_ignored() {
    let src = "// use crate::adapter::A;\n/* crate::adapter::B */\n/// crate::adapter::C\nfn f() -> &'static str { \"crate::adapter::D\" }";
    assert!(bad(src, "domain").is_empty());
}

#[test]
fn inline_mod_extends_module_path() {
    assert!(bad("mod inner { use super::adapter::X; }", "domain").is_empty());
    assert_eq!(
        bad("mod inner { use super::super::adapter::X; }", "domain"),
        vec![m("adapter::X")]
    );
}

#[test]
fn files_map_to_module_paths() {
    let src = Path::new("/c/src");
    assert_eq!(
        file_module(src, Path::new("/c/src/lib.rs")),
        Vec::<String>::new()
    );
    assert_eq!(file_module(src, Path::new("/c/src/a/mod.rs")), m("a"));
    assert_eq!(file_module(src, Path::new("/c/src/a/b.rs")), m("a::b"));
}

#[test]
fn rule_table_parses() {
    let r = parse_rules("# c\nfake::domain\tfake::util, fake::x::y\nfake::solo\n").unwrap();
    assert_eq!(r[0].allowed, vec![m("util"), m("x::y")]);
    assert!(r[1].allowed.is_empty());
}
