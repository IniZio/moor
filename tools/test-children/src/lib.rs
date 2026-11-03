//! Test code must not hold a spawned child outside `moor_testkit::ChildGuard`.
//!
//! A bare `std::process::Child` is not killed when dropped, so a test that panics before its
//! explicit kill leaves the process running. A `.spawn()` in test code is accepted only when its
//! child goes straight into `ChildGuard::new(..)`, into `pump(..)` (which owns and reaps it), or is
//! waited on in the same expression.

/// How to clear a failure, printed after the violations.
pub const FIX: &str = "fix: wrap the child in moor_testkit::ChildGuard::new(..) so a panic kills it, or \
read its output with .output() or .status(). A file that cannot (it needs no test process of its own) \
goes in tools/test-children.allow with a reason.";

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{
    Attribute, Block, Expr, ExprCall, ExprMethodCall, File, Item, ItemFn, ItemMod, Pat, Stmt,
};

/// Methods that hand the same child on, so `cmd.spawn().unwrap()` still counts as the spawn.
const PASS_THROUGH: [&str; 5] = ["unwrap", "expect", "map_err", "unwrap_or_else", "context"];
/// Methods that wait for the child within the expression that spawned it.
const WAITS: [&str; 2] = ["wait", "wait_with_output"];

/// True for `#[cfg(test)]` and `#[test]`.
fn is_test_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && a.parse_args::<syn::Path>()
                    .is_ok_and(|p| p.is_ident("test")))
    })
}

fn is_spawn(m: &ExprMethodCall) -> bool {
    m.method == "spawn" && m.args.is_empty()
}

/// The `.spawn()` call that `e` is, looking through `?`, parentheses and pass-through methods.
fn spawn_of(e: &Expr) -> Option<&ExprMethodCall> {
    match e {
        Expr::Try(t) => spawn_of(&t.expr),
        Expr::Paren(p) => spawn_of(&p.expr),
        Expr::Group(g) => spawn_of(&g.expr),
        Expr::MethodCall(m) if is_spawn(m) => Some(m),
        Expr::MethodCall(m) if PASS_THROUGH.iter().any(|n| m.method == n) => spawn_of(&m.receiver),
        _ => None,
    }
}

/// `ChildGuard::new`, or `pump`, which takes the child by value and kills and reaps it itself.
fn ends_with_guard_new(p: &syn::Path) -> bool {
    let segs: Vec<_> = p.segments.iter().map(|s| s.ident.to_string()).collect();
    matches!(segs.as_slice(), [.., a, b] if a == "ChildGuard" && b == "new")
        || matches!(segs.as_slice(), [.., last] if last == "pump")
}

fn key(m: &ExprMethodCall) -> (usize, usize) {
    let start = m.method.span().start();
    (start.line, start.column)
}

/// True when `ChildGuard::new(name)` appears anywhere in `stmts`.
fn wraps_later(name: &syn::Ident, stmts: &[Stmt]) -> bool {
    struct Wraps<'a>(&'a syn::Ident, bool);
    impl<'ast> Visit<'ast> for Wraps<'_> {
        fn visit_expr_call(&mut self, c: &'ast ExprCall) {
            let named = c
                .args
                .iter()
                .any(|a| matches!(a, Expr::Path(p) if p.path.is_ident(self.0)));
            if named && matches!(&*c.func, Expr::Path(p) if ends_with_guard_new(&p.path)) {
                self.1 = true;
            }
            syn::visit::visit_expr_call(self, c);
        }
    }
    let mut w = Wraps(name, false);
    for s in stmts {
        w.visit_stmt(s);
    }
    w.1
}

struct Finder {
    in_test: bool,
    accepted: HashSet<(usize, usize)>,
    lines: Vec<usize>,
}

impl Finder {
    fn accept(&mut self, e: &Expr) {
        if let Some(m) = spawn_of(e) {
            self.accepted.insert(key(m));
        }
    }

    fn scoped(&mut self, test: bool, f: impl FnOnce(&mut Self)) {
        let before = self.in_test;
        self.in_test = before || test;
        f(self);
        self.in_test = before;
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_item(&mut self, i: &'ast Item) {
        let test = match i {
            Item::Fn(x) => is_test_attr(&x.attrs),
            Item::Mod(x) => is_test_attr(&x.attrs),
            Item::Impl(x) => is_test_attr(&x.attrs),
            _ => false,
        };
        self.scoped(test, |s| syn::visit::visit_item(s, i));
    }

    fn visit_item_fn(&mut self, i: &'ast ItemFn) {
        self.scoped(is_test_attr(&i.attrs), |s| syn::visit::visit_item_fn(s, i));
    }

    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        self.scoped(is_test_attr(&i.attrs), |s| syn::visit::visit_item_mod(s, i));
    }

    fn visit_block(&mut self, b: &'ast Block) {
        for (i, stmt) in b.stmts.iter().enumerate() {
            let Stmt::Local(local) = stmt else { continue };
            let bound = match &local.pat {
                Pat::Ident(p) => Some(&p.ident),
                _ => None,
            };
            let spawned = local.init.as_ref().and_then(|init| spawn_of(&init.expr));
            if let (Some(name), Some(spawn)) = (bound, spawned) {
                if wraps_later(name, &b.stmts[i + 1..]) {
                    self.accepted.insert(key(spawn));
                }
            }
        }
        syn::visit::visit_block(self, b);
    }

    fn visit_expr_call(&mut self, c: &'ast ExprCall) {
        if matches!(&*c.func, Expr::Path(p) if ends_with_guard_new(&p.path)) {
            for arg in &c.args {
                self.accept(arg);
            }
        }
        syn::visit::visit_expr_call(self, c);
    }

    fn visit_expr_method_call(&mut self, m: &'ast ExprMethodCall) {
        if WAITS.iter().any(|n| m.method == n) {
            self.accept(&m.receiver);
        }
        if m.method == "map" && m.args.len() == 1 {
            let guarded =
                matches!(m.args.first(), Some(Expr::Path(p)) if ends_with_guard_new(&p.path));
            if guarded {
                self.accept(&m.receiver);
            }
        }
        if self.in_test && is_spawn(m) && !self.accepted.contains(&key(m)) {
            self.lines.push(m.method.span().start().line);
        }
        syn::visit::visit_expr_method_call(self, m);
    }
}

/// Lines of the unguarded `.spawn()` calls in `file`, counting all of it as test code when
/// `whole_file_is_test` is set and only its `#[cfg(test)]` and `#[test]` items otherwise.
#[must_use]
pub fn unguarded_spawns(file: &File, whole_file_is_test: bool) -> Vec<usize> {
    let mut f = Finder {
        in_test: whole_file_is_test || is_test_attr(&file.attrs),
        accepted: HashSet::new(),
        lines: Vec::new(),
    };
    f.visit_file(file);
    f.lines
}

/// Files that a `#[cfg(test)] mod name;` declaration in `file` (at `path`) points to.
fn test_mod_files(file: &File, path: &Path) -> Vec<PathBuf> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let dir = path.parent().unwrap_or(Path::new(""));
    let base = if matches!(stem.as_deref(), Some("mod" | "lib" | "main")) {
        dir.to_path_buf()
    } else {
        dir.join(stem.unwrap_or_default())
    };
    file.items
        .iter()
        .filter_map(|i| match i {
            Item::Mod(m) if m.content.is_none() && is_test_attr(&m.attrs) => {
                let name = m.ident.to_string();
                let flat = base.join(format!("{name}.rs"));
                Some(if flat.exists() {
                    flat
                } else {
                    base.join(&name).join("mod.rs")
                })
            }
            _ => None,
        })
        .collect()
}

/// Sources written for tests by their place: `tests.rs`, anything under a `tests` or `testing` directory.
fn is_test_path(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n == "tests.rs")
        || p.components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "testing")))
}

fn rs_files(dir: &Path, out: &mut BTreeSet<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            if p.file_name().is_none_or(|n| n != "target") {
                rs_files(&p, out)?;
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.insert(p);
        }
    }
    Ok(())
}

/// Allowlist entries: one repo-relative file per line, `#` starts a comment.
fn allowed(allow: &str) -> BTreeSet<&str> {
    allow
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect()
}

/// True when `file` is `root` or lies in the module directory of the out-of-line test module `root`.
fn under(file: &Path, root: &Path) -> bool {
    file == root
        || root.parent().is_some_and(|dir| {
            let is_mod = root.file_name().is_some_and(|n| n == "mod.rs");
            let own = if is_mod {
                dir.to_path_buf()
            } else {
                root.with_extension("")
            };
            file.starts_with(own)
        })
}

/// Scans `crates/` and `tools/` under `root`; returns the number of test files read and one
/// message per unguarded spawn. `allow` lists files exempt from the check.
///
/// # Errors
/// Returns a message when a source file cannot be read or parsed, or an allowlist entry names a file
/// that is gone or has nothing to exempt.
pub fn check(root: &Path, allow: &str) -> Result<(usize, Vec<String>), String> {
    let mut files = BTreeSet::new();
    for top in ["crates", "tools"]
        .map(|t| root.join(t))
        .iter()
        .filter(|t| t.is_dir())
    {
        rs_files(top, &mut files).map_err(|e| format!("{}: {e}", top.display()))?;
    }
    let mut parsed = Vec::new();
    for f in &files {
        let text = fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let ast = syn::parse_file(&text).map_err(|e| format!("{}: {e}", f.display()))?;
        parsed.push((f, ast));
    }
    let test_roots: Vec<PathBuf> = parsed
        .iter()
        .flat_map(|(f, a)| test_mod_files(a, f))
        .collect();
    let allow = allowed(allow);
    let mut used = BTreeSet::new();
    let (mut bad, mut tested) = (Vec::new(), 0);
    for (f, ast) in &parsed {
        let whole = is_test_path(f) || test_roots.iter().any(|r| under(f, r));
        let rel = f.strip_prefix(root).unwrap_or(f);
        let lines = unguarded_spawns(ast, whole);
        tested += usize::from(whole);
        if let Some(name) = rel.to_str().filter(|n| allow.contains(n)) {
            if lines.is_empty() {
                return Err(format!(
                    "{name} is in tools/test-children.allow but has no unguarded spawn"
                ));
            }
            used.insert(name.to_owned());
            continue;
        }
        bad.extend(lines.into_iter().map(|l| {
            format!(
                "test-children: {}:{l} spawns a child that is not in a ChildGuard",
                rel.display()
            )
        }));
    }
    if let Some(gone) = allow.iter().find(|a| !used.contains(**a)) {
        return Err(format!(
            "{gone} is in tools/test-children.allow but is not a file to check"
        ));
    }
    Ok((tested, bad))
}

#[cfg(test)]
mod tests;
