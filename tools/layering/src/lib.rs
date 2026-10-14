//! Module layering check: which crate modules may name which others.

mod confine;

pub use confine::{escapes, Confine};

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

use syn::{Attribute, ImplItemFn, Item, ItemMod, ItemUse, UseTree};

/// One path mention: `crate::`/`self::`/`super::` resolved to crate-relative
/// segments, or any other first segment kept as an external identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Source line of the first path segment.
    pub line: usize,
    /// Module containing the mention.
    pub from: Vec<String>,
    /// Module path the mention names, below the crate root (empty when `ext` is set).
    pub to: Vec<String>,
    /// First segment when it is neither `crate`, `self` nor `super`.
    pub ext: Option<String>,
}

/// A layering rule: `module` may use only `allowed` (plus its own tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Crate the rule applies to.
    pub krate: String,
    /// Constrained module, crate-relative.
    pub module: Vec<String>,
    /// Permitted modules, crate-relative.
    pub allowed: Vec<Vec<String>>,
}

/// Splits a comma-separated module list, dropping blanks.
pub(crate) fn list(cell: &str) -> impl Iterator<Item = &str> {
    cell.split(',').map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn split(path: &str) -> Vec<String> {
    path.split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parses the tab-separated rule table.
///
/// The first line must be the header `module<TAB>allowed`. Each later line is
/// `module<TAB>allowed`: module paths are crate-relative (`moor_core::name`),
/// `allowed` is a comma-separated list of modules it may use via `crate::`
/// (empty = none). A module not listed is unconstrained; a module may always
/// use itself. Blank lines are skipped.
///
/// A row `extern:<ident><TAB>a::b,c::d` confines the external crate `<ident>`
/// (`nono_proxy`): any path starting with it, in the crate of the listed
/// modules, is a violation outside those modules and their trees. The list
/// must be non-empty and name one crate.
///
/// # Errors
/// Returns the offending line when the header is missing, a row does not have
/// exactly two columns, a module is not `crate::path` shaped, or an `extern:`
/// row has an empty ident, an empty list or modules from several crates.
pub fn parse_rules(table: &str) -> Result<(Vec<Rule>, Vec<Confine>), String> {
    let mut lines = table.lines();
    if lines.next() != Some("module\tallowed") {
        return Err("bad header: first line must be `module<TAB>allowed`".to_string());
    }
    let mut rules = Vec::new();
    let mut confines = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let [module, allowed] = cols[..] else {
            return Err(format!(
                "bad rule (want 2 tab-separated columns, got {}): {line}",
                cols.len()
            ));
        };
        if let Some(ident) = module.trim().strip_prefix("extern:") {
            confines
                .push(Confine::parse(ident, allowed).ok_or(format!("bad extern rule: {line}"))?);
            continue;
        }
        let mut full = split(module.trim());
        if full.is_empty() {
            return Err(format!("bad rule: {line}"));
        }
        let krate = full.remove(0);
        let allowed = list(allowed)
            .map(|s| split(s).into_iter().skip(1).collect())
            .collect();
        rules.push(Rule {
            krate,
            module: full,
            allowed,
        });
    }
    Ok((rules, confines))
}

struct Collector {
    module: Vec<String>,
    out: Vec<Ref>,
}

/// True for exactly `#[cfg(test)]`; `cfg(not(test))` and `cfg(any(test, ..))` are not.
fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Path>()
                .is_ok_and(|p| p.is_ident("test"))
    })
}

fn item_attrs(i: &Item) -> &[Attribute] {
    match i {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
}

impl Collector {
    fn resolve(&self, segs: &[String]) -> Option<Vec<String>> {
        let (first, mut rest) = segs.split_first()?;
        let mut base = match first.as_str() {
            "crate" => Vec::new(),
            "self" => self.module.clone(),
            "super" => {
                let mut m = self.module.clone();
                m.pop()?;
                while let Some((s, r)) = rest.split_first() {
                    if s != "super" {
                        break;
                    }
                    m.pop()?;
                    rest = r;
                }
                m
            }
            _ => return None,
        };
        base.extend(rest.iter().cloned());
        Some(base)
    }

    fn push(&mut self, line: usize, segs: &[String]) {
        let (to, ext) = match self.resolve(segs) {
            Some(to) => (to, None),
            None => (Vec::new(), segs.first().cloned()),
        };
        self.out.push(Ref {
            line,
            from: self.module.clone(),
            to,
            ext,
        });
    }

    fn tree(&mut self, prefix: &mut Vec<String>, line: usize, t: &UseTree) {
        match t {
            UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.tree(prefix, p.ident.span().start().line, &p.tree);
                prefix.pop();
            }
            UseTree::Name(n) => {
                self.leaf(prefix, &n.ident.to_string(), n.ident.span().start().line)
            }
            UseTree::Rename(r) => {
                self.leaf(prefix, &r.ident.to_string(), r.ident.span().start().line)
            }
            UseTree::Glob(_) => self.push(line, prefix),
            UseTree::Group(g) => {
                for item in &g.items {
                    self.tree(prefix, line, item);
                }
            }
        }
    }

    fn leaf(&mut self, prefix: &[String], name: &str, line: usize) {
        let mut full = prefix.to_vec();
        if name != "self" {
            full.push(name.to_owned());
        }
        self.push(line, &full);
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item(&mut self, i: &'ast Item) {
        if !is_cfg_test(item_attrs(i)) {
            syn::visit::visit_item(self, i);
        }
    }

    fn visit_impl_item_fn(&mut self, i: &'ast ImplItemFn) {
        if !is_cfg_test(&i.attrs) {
            syn::visit::visit_impl_item_fn(self, i);
        }
    }

    fn visit_item_use(&mut self, i: &'ast ItemUse) {
        let line = i.use_token.span.start().line;
        self.tree(&mut Vec::new(), line, &i.tree);
    }

    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        self.module.push(i.ident.to_string());
        syn::visit::visit_item_mod(self, i);
        self.module.pop();
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        if let Some(first) = p.segments.first() {
            self.push(first.ident.span().start().line, &segs);
        }
        syn::visit::visit_path(self, p);
    }
}

/// Collects every resolved mention in `src`, a file whose module is `file_module`.
/// Test-only code (`#[cfg(test)]`) is skipped.
///
/// # Errors
/// Returns the syn parse error when `src` is not valid Rust.
pub fn refs(src: &str, file_module: &[String]) -> Result<Vec<Ref>, syn::Error> {
    let file = syn::parse_file(src)?;
    let mut c = Collector {
        module: file_module.to_vec(),
        out: Vec::new(),
    };
    if !is_cfg_test(&file.attrs) {
        c.visit_file(&file);
    }
    Ok(c.out)
}

pub(crate) fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a == b)
}

/// Mentions that break `rule`: from inside its module, to a sibling not allowed.
#[must_use]
pub fn violations<'a>(all: &'a [Ref], rule: &Rule) -> Vec<&'a Ref> {
    all.iter()
        .filter(|r| r.ext.is_none() && starts_with(&r.from, &rule.module))
        .filter(|r| {
            let related = starts_with(&r.to, &rule.module) || starts_with(&rule.module, &r.to);
            !related && !rule.allowed.iter().any(|a| starts_with(&r.to, a))
        })
        .collect()
}

/// Test-only sources: a `tests.rs` file or a `tests` directory.
fn is_test_path(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n == "tests.rs" || (n == "tests" && p.is_dir()))
}

fn rs_files(dir: &Path, out: &mut BTreeSet<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            if !is_test_path(&p) {
                rs_files(&p, out)?;
            }
        } else if p.extension().is_some_and(|e| e == "rs") && !is_test_path(&p) {
            out.insert(p);
        }
    }
    Ok(())
}

/// Module path of a file below `src`: `a/mod.rs` and `a.rs` are `a`; `lib.rs` is the root.
#[must_use]
pub fn file_module(src: &Path, file: &Path) -> Vec<String> {
    let rel = file.strip_prefix(src).unwrap_or(file).with_extension("");
    let mut segs: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if matches!(
        segs.last().map(String::as_str),
        Some("mod" | "lib" | "main")
    ) {
        segs.pop();
    }
    segs
}

fn crate_dir(root: &Path, name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for parent in [root.to_path_buf(), root.join("crates"), root.join("tools")] {
        if let Ok(rd) = fs::read_dir(parent) {
            dirs.extend(rd.filter_map(Result::ok).map(|e| e.path()));
        }
    }
    dirs.into_iter().find(|d| {
        let want = format!("name=\"{}\"", name.replace('-', "_"));
        fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|t| {
            t.lines()
                .any(|l| l.replace(' ', "").replace('-', "_") == want)
        })
    })
}

/// Checks every rule against the sources under `root`; returns one message per violation.
///
/// # Errors
/// Returns a message when a crate directory, a source file or a rule cannot be read or parsed.
pub fn check(root: &Path, table: &str) -> Result<(usize, Vec<String>), String> {
    let (rules, confines) = parse_rules(table)?;
    let all_krates = rules
        .iter()
        .map(|r| &r.krate)
        .chain(confines.iter().map(|c| &c.krate));
    let crates: BTreeSet<&str> = all_krates.map(String::as_str).collect();
    let mut bad = Vec::new();
    for krate in crates {
        let Some(dir) = crate_dir(root, krate) else {
            return Err(format!("no crate dir for {krate}"));
        };
        let src = dir.join("src");
        let mut files = BTreeSet::new();
        rs_files(&src, &mut files).map_err(|e| format!("{}: {e}", src.display()))?;
        let mut all = Vec::new();
        let mut owner = Vec::new();
        for f in &files {
            let text = fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
            let found =
                refs(&text, &file_module(&src, f)).map_err(|e| format!("{}: {e}", f.display()))?;
            owner.extend(std::iter::repeat_n(f.clone(), found.len()));
            all.extend(found);
        }
        let mut report = |r: &Ref, what: String| {
            let idx = all.iter().position(|x| std::ptr::eq(x, r)).unwrap_or(0);
            let file = owner[idx].strip_prefix(root).unwrap_or(&owner[idx]);
            bad.push(format!("layering: {}:{} {what}", file.display(), r.line));
        };
        for c in confines.iter().filter(|c| c.krate == krate) {
            let within: Vec<String> = c.within.iter().map(|w| w.join("::")).collect();
            for r in escapes(&all, c) {
                let from = r.from.join("::");
                let to = within.join(", ");
                report(
                    r,
                    format!("{krate}::{from} names {}, confined to {to}", c.ident),
                );
            }
        }
        for rule in rules.iter().filter(|r| r.krate == krate) {
            for r in violations(&all, rule) {
                let (m, to) = (rule.module.join("::"), r.to.join("::"));
                report(r, format!("{krate}::{m} uses {krate}::{to}"));
            }
        }
    }
    Ok((rules.len() + confines.len(), bad))
}

#[cfg(test)]
mod tests;
