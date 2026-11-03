//! Fails when this branch's store migrations collide with the base branch's.
//!
//! Two branches that each add the next migration number both pass their own tests, and the second
//! to merge has to be renumbered by hand. A renumbered migration that stores already applied
//! changes its pinned checksum and breaks them, so the collision is caught before the merge.

use std::collections::BTreeMap;

use syn::visit::Visit;
use syn::{Expr, ExprStruct, Item, Lit, Member};

/// Where the store's migration list is written.
pub const STEPS_FILE: &str = "crates/moor-schema/src/lib.rs";

/// How to clear a failure, printed after the violations.
pub const FIX: &str = "fix: give each new migration a version above the base branch's highest, \
and never rename or renumber one the base already has. Rebase onto the base branch, then renumber \
only the steps this branch added.";

/// The version in a migration name `V<n>__<name>`.
fn version(name: &str) -> Result<u32, String> {
    name.strip_prefix('V')
        .and_then(|rest| rest.split_once("__"))
        .and_then(|(n, _)| n.parse().ok())
        .ok_or_else(|| format!("migration `{name}` is not named V<n>__<name>"))
}

/// Collects the `name` of each `Step { .. }` literal it visits.
struct Names(Vec<String>);

impl<'ast> Visit<'ast> for Names {
    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        for field in &node.fields {
            if matches!(&field.member, Member::Named(id) if id == "name") {
                if let Expr::Lit(lit) = &field.expr {
                    if let Lit::Str(name) = &lit.lit {
                        self.0.push(name.value());
                    }
                }
            }
        }
    }
}

/// The migration names in the `STEPS` list of `source`, the text of [`STEPS_FILE`].
pub fn step_names(source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|e| format!("cannot parse {STEPS_FILE}: {e}"))?;
    let mut names = Names(Vec::new());
    for item in &file.items {
        if let Item::Const(c) = item {
            if c.ident == "STEPS" {
                names.visit_expr(&c.expr);
            }
        }
    }
    if names.0.is_empty() {
        return Err(format!(
            "no `STEPS` list of `Step {{ name: .. }}` in {STEPS_FILE}"
        ));
    }
    Ok(names.0)
}

/// What is wrong with `head` alone: a version out of order or used twice.
fn order(head: &[(u32, &str)]) -> Vec<String> {
    head.windows(2)
        .filter(|w| w[1].0 <= w[0].0)
        .map(|w| {
            if w[1].0 == w[0].0 {
                format!(
                    "version {} is used twice: {} and {}",
                    w[0].0, w[0].1, w[1].1
                )
            } else {
                format!(
                    "{} is listed after {}, but versions must increase down the list",
                    w[1].1, w[0].1
                )
            }
        })
        .collect()
}

/// Every way `head`, this branch's migration names, collides with `base`, the names on `base_ref`.
pub fn violations(base_ref: &str, base: &[String], head: &[&str]) -> Result<Vec<String>, String> {
    let head: Vec<(u32, &str)> = head
        .iter()
        .map(|n| version(n).map(|v| (v, *n)))
        .collect::<Result<_, _>>()?;
    let base: BTreeMap<u32, &str> = base
        .iter()
        .map(|n| version(n).map(|v| (v, n.as_str())))
        .collect::<Result<_, _>>()?;
    let mut bad = order(&head);
    let top = base.last_key_value();
    for &(v, name) in &head {
        match base.get(&v) {
            Some(&theirs) if theirs != name => bad.push(format!(
                "{name} collides with {theirs} on {base_ref}: both are version {v}"
            )),
            Some(_) => {}
            None => {
                if let Some((&high, &highest)) = top.filter(|(&high, _)| v < high) {
                    bad.push(format!(
                        "{name} is new on this branch but below {highest} on {base_ref}: \
                         a new migration must be above version {high}"
                    ));
                }
            }
        }
    }
    Ok(bad)
}

#[cfg(test)]
mod tests;
