//! The Mac layout a Box's lease paths name, for a Task whose Box workdir was never recorded.

use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::BoxPath;

use crate::store::lease::{LeaseKind, LeaseRow};

/// The Mac layout the lease paths name: a Box path below one of the layout directories
/// (`work`, `home`, `tmp`, `cache`, `sim`, `launch`) of `<work_root>/<key>`.
///
/// A path equal to the root itself proves nothing, since any Box could hold a directory named
/// after the key. Only whole path components match, so a path of another Task whose key starts
/// with this one gives nothing. When a path holds the key more than once the last fit wins, and
/// rows that name different roots give nothing: a wrong root would remove the wrong directory.
pub(super) fn layout_from_leases(rows: &[LeaseRow], key: &TaskKey) -> Option<MacLayout> {
    let mut found: Option<MacLayout> = None;
    for r in rows.iter().filter(|r| {
        matches!(
            r.kind,
            LeaseKind::BoxRoot | LeaseKind::BoxDir | LeaseKind::Home
        )
    }) {
        let Some(layout) = layout_of_path(&r.key, key) else {
            continue;
        };
        match &found {
            Some(f) if f.root != layout.root => return None,
            _ => found = Some(layout),
        }
    }
    found
}

/// The layout of `key` that `path` lies under.
///
/// The last fit wins: `/w/k/home/.cache/k` is below `/w/k/home`, not below
/// `/w/k/home/.cache/k/home`.
fn layout_of_path(path: &str, key: &TaskKey) -> Option<MacLayout> {
    if !path.starts_with('/') {
        return None;
    }
    let marker = format!("/{}", key.as_str());
    let within = |dir: &BoxPath| {
        path.strip_prefix(dir.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    path.match_indices(&marker)
        .filter_map(|(i, _)| {
            let parent = path.get(..i).filter(|p| !p.is_empty()).unwrap_or("/");
            MacLayout::new(&BoxPath::restore(parent).ok()?, key).ok()
        })
        .filter(|l| {
            [&l.work, &l.home, &l.tmp, &l.cache, &l.sim, &l.launch]
                .into_iter()
                .any(within)
        })
        .last()
}

#[cfg(test)]
mod tests;
