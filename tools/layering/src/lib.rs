//! Module layering check: which crate modules may name which others.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{ItemMod, ItemUse, UseTree};

/// One `crate::`, `self::` or `super::` mention, resolved to crate-relative segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Source line of the first path segment.
    pub line: usize,
    /// Module containing the mention.
    pub from: Vec<String>,
    /// Module path the mention names, below the crate root.
    pub to: Vec<String>,
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

fn split(path: &str) -> Vec<String> {
    path.split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parses the tab-separated rule table; `#` lines and blanks are skipped.
///
/// # Errors
/// Returns the offending line when a module is not `crate::path` shaped.
pub fn parse_rules(table: &str) -> Result<Vec<Rule>, String> {
    let mut rules = Vec::new();
    for line in table.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (module, allowed) = line.split_once('\t').unwrap_or((line, ""));
        let mut full = split(module.trim());
        if full.is_empty() {
            return Err(format!("bad rule: {line}"));
        }
        let krate = full.remove(0);
        let allowed = allowed
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| split(s).into_iter().skip(1).collect())
            .collect();
        rules.push(Rule {
            krate,
            module: full,
            allowed,
        });
    }
    Ok(rules)
}

struct Collector {
    module: Vec<String>,
    out: Vec<Ref>,
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
        if let Some(to) = self.resolve(segs) {
            self.out.push(Ref {
                line,
                from: self.module.clone(),
                to,
            });
        }
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
///
/// # Errors
/// Returns the syn parse error when `src` is not valid Rust.
pub fn refs(src: &str, file_module: &[String]) -> Result<Vec<Ref>, syn::Error> {
    let file = syn::parse_file(src)?;
    let mut c = Collector {
        module: file_module.to_vec(),
        out: Vec::new(),
    };
    c.visit_file(&file);
    Ok(c.out)
}

fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a == b)
}

/// Mentions that break `rule`: from inside its module, to a sibling not allowed.
#[must_use]
pub fn violations<'a>(all: &'a [Ref], rule: &Rule) -> Vec<&'a Ref> {
    let depth = rule.module.len();
    all.iter()
        .filter(|r| starts_with(&r.from, &rule.module))
        .filter(|r| {
            let to = &r.to[..r.to.len().min(depth)];
            let related = starts_with(to, &rule.module) || starts_with(&rule.module, to);
            !related && !rule.allowed.iter().any(|a| a == to)
        })
        .collect()
}

fn rs_files(dir: &Path, out: &mut BTreeSet<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            rs_files(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
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
    let rules = parse_rules(table)?;
    let mut bad = Vec::new();
    for rule in &rules {
        let dir = crate_dir(root, &rule.krate)
            .ok_or_else(|| format!("no crate dir for {}", rule.krate))?;
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
        for r in violations(&all, rule) {
            let idx = all.iter().position(|x| std::ptr::eq(x, r)).unwrap_or(0);
            bad.push(format!(
                "layering: {}:{} {}::{} uses {}::{}",
                owner[idx]
                    .strip_prefix(root)
                    .unwrap_or(&owner[idx])
                    .display(),
                r.line,
                rule.krate,
                rule.module.join("::"),
                rule.krate,
                r.to.join("::"),
            ));
        }
    }
    Ok((rules.len(), bad))
}

#[cfg(test)]
mod tests;
