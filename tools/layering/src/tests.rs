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
    let r = parse_rules("module\tallowed\nfake::domain\tfake::util, fake::x::y\nfake::solo\t\n")
        .unwrap()
        .0;
    assert_eq!(r[0].allowed, vec![m("util"), m("x::y")]);
    assert!(r[1].allowed.is_empty());
}

#[test]
fn header_is_required() {
    assert!(parse_rules("fake::solo\t\n")
        .unwrap_err()
        .contains("header"));
    assert!(parse_rules("").unwrap_err().contains("header"));
}

#[test]
fn bad_column_count_is_rejected() {
    assert!(parse_rules("module\tallowed\nfake::solo\n")
        .unwrap_err()
        .contains("got 1"));
    assert!(parse_rules("module\tallowed\na::b\tc\td\n")
        .unwrap_err()
        .contains("got 3"));
}

#[test]
fn crate_dir_matches_hyphen_and_underscore() {
    let root = std::env::temp_dir().join(format!("layering-crate-dir-{}", std::process::id()));
    let dir = root.join("crates").join("moor-core");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"moor-core\"\n").unwrap();
    assert_eq!(crate_dir(&root, "moor_core"), Some(dir.clone()));
    assert_eq!(crate_dir(&root, "moor-core"), Some(dir));
    assert_eq!(crate_dir(&root, "other"), None);
    fs::remove_dir_all(&root).unwrap();
}

fn bad_with(src: &str, file_mod: &str, rule: &Rule) -> Vec<Vec<String>> {
    let all = refs(src, &m(file_mod)).unwrap();
    violations(&all, rule)
        .into_iter()
        .map(|r| r.to.clone())
        .collect()
}

fn deep_rule() -> Rule {
    Rule {
        krate: "fake".into(),
        module: m("ops"),
        allowed: vec![m("adapters::git")],
    }
}

#[test]
fn deeper_allowed_entry_permits_its_subtree() {
    let got = bad_with("use crate::adapters::git::Repo;", "ops", &deep_rule());
    assert!(got.is_empty());
    assert!(bad_with("use crate::adapters::git;", "ops", &deep_rule()).is_empty());
}

#[test]
fn deeper_allowed_entry_does_not_permit_sibling() {
    let got = bad_with(
        "use crate::adapters::{git::Repo, crabbox::Box};",
        "ops",
        &deep_rule(),
    );
    assert_eq!(got, vec![m("adapters::crabbox::Box")]);
}

#[test]
fn parent_of_allowed_entry_is_flagged() {
    let got = bad_with("use crate::adapters;", "ops", &deep_rule());
    assert_eq!(got, vec![m("adapters")]);
}

#[test]
fn shallow_allowed_entry_permits_leaf_items() {
    assert!(bad("use crate::util::Store;", "domain").is_empty());
}

#[test]
fn cfg_test_mod_is_skipped() {
    let src = "#[cfg(test)]\nmod t { use crate::adapter::A; }";
    assert!(bad(src, "domain").is_empty());
}

#[test]
fn cfg_test_fn_use_and_impl_are_skipped() {
    let src = "#[cfg(test)]\nfn f() { crate::adapter::g(); }\n#[cfg(test)]\nuse crate::adapter::U;\nstruct S;\n#[cfg(test)]\nimpl S { fn h() { crate::adapter::h(); } }\nimpl S { #[cfg(test)] fn k() { crate::adapter::k(); } }";
    assert!(bad(src, "domain").is_empty());
}

#[test]
fn cfg_not_test_and_any_test_still_count() {
    let src = "#[cfg(not(test))]\nuse crate::adapter::A;\n#[cfg(any(test, unix))]\nuse crate::adapter::B;";
    assert_eq!(bad(src, "domain"), vec![m("adapter::A"), m("adapter::B")]);
}

#[test]
fn tests_rs_and_tests_dir_are_skipped() {
    assert!(is_test_path(Path::new("/c/src/store/tests.rs")));
    assert!(!is_test_path(Path::new("/c/src/store/mod.rs")));
    let root = std::env::temp_dir().join(format!("layering-tests-skip-{}", std::process::id()));
    fs::create_dir_all(root.join("tests")).unwrap();
    for f in ["a.rs", "tests.rs", "tests/b.rs"] {
        fs::write(root.join(f), "").unwrap();
    }
    let mut got = BTreeSet::new();
    rs_files(&root, &mut got).unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(got, BTreeSet::from([root.join("a.rs")]));
}

const ROW: &str = "module\tallowed\nextern:nono_proxy\tfake::proxy, fake::x::y\n";

fn confine() -> Confine {
    parse_rules(ROW).unwrap().1.remove(0)
}

fn escaped(src: &str, file_mod: &str) -> Vec<usize> {
    let all = refs(src, &m(file_mod)).unwrap();
    escapes(&all, &confine()).iter().map(|r| r.line).collect()
}

#[test]
fn extern_row_parses() {
    let c = confine();
    assert_eq!((c.krate.as_str(), c.ident.as_str()), ("fake", "nono_proxy"));
    assert_eq!(c.within, vec![m("proxy"), m("x::y")]);
}

#[test]
fn extern_use_and_path_allowed_inside_module_tree() {
    let src = "use nono_proxy::Config;\nfn f() { nono_proxy::start(); }";
    assert!(escaped(src, "proxy").is_empty());
    assert!(escaped(src, "proxy::nono").is_empty());
    assert!(escaped(src, "x::y::z").is_empty());
}

#[test]
fn extern_use_and_path_flagged_elsewhere() {
    let src = "use nono_proxy::{A, B};\nfn f() { nono_proxy::start(); }\nuse other::C;";
    assert_eq!(escaped(src, "wire"), vec![1, 1, 2]);
    assert_eq!(escaped(src, "x"), vec![1, 1, 2]);
}

#[test]
fn extern_absolute_path_is_flagged() {
    assert_eq!(escaped("fn f() { ::nono_proxy::go(); }", "wire"), vec![1]);
}

#[test]
fn extern_in_cfg_test_is_skipped() {
    let src =
        "#[cfg(test)]\nmod t { use nono_proxy::A; }\n#[cfg(test)]\nfn f() { nono_proxy::go(); }";
    assert!(escaped(src, "wire").is_empty());
}

#[test]
fn extern_does_not_trip_module_rules() {
    assert!(bad("use nono_proxy::A;", "domain").is_empty());
}

#[test]
fn bad_extern_rows_are_rejected() {
    for row in [
        "extern:\tfake::proxy",
        "extern:nono\t",
        "extern:nono\tfake",
        "extern:nono\tfake::a, other::b",
    ] {
        let err = parse_rules(&format!("module\tallowed\n{row}\n")).unwrap_err();
        assert!(err.contains("bad extern rule"), "{row}: {err}");
    }
}
