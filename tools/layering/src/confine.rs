//! Confinement of external crates to named modules.

use crate::{list, split, starts_with, Ref};

/// A confinement: paths starting with `ident` may appear only within `within`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confine {
    /// Crate the confinement applies to.
    pub krate: String,
    /// External crate identifier, as written in source (`nono_proxy`).
    pub ident: String,
    /// Modules allowed to name it, crate-relative (their trees included).
    pub within: Vec<Vec<String>>,
}

impl Confine {
    /// Builds one from an `extern:<ident>` row; `None` unless `allowed` names
    /// at least one module and all of them (as `krate::path`) share one crate.
    #[must_use]
    pub fn parse(ident: &str, allowed: &str) -> Option<Self> {
        let ident = ident.trim();
        let mods: Vec<Vec<String>> = list(allowed).map(split).collect();
        let krate = mods.first()?.first()?.clone();
        let rest = mods
            .iter()
            .map(|m| {
                m.split_first()
                    .filter(|(k, r)| **k == krate && !r.is_empty())
            })
            .collect::<Option<Vec<_>>>()?;
        (!ident.is_empty()).then(|| Self {
            krate,
            ident: ident.to_owned(),
            within: rest.into_iter().map(|(_, r)| r.to_vec()).collect(),
        })
    }
}

/// Mentions of the confined crate from outside the modules allowed to name it.
#[must_use]
pub fn escapes<'a>(all: &'a [Ref], c: &Confine) -> Vec<&'a Ref> {
    all.iter()
        .filter(|r| r.ext.as_deref() == Some(c.ident.as_str()))
        .filter(|r| !c.within.iter().any(|w| starts_with(&r.from, w)))
        .collect()
}
