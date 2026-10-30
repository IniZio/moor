//! Learns a Box's workdir from what crabbox reports when it leases the Box.

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::seam::proc::ProcSpec;

use super::errors::{map_error, unreadable, Op};
use super::progress::Watch;
use super::{cache, handle_of, parse, size_of, slug, split_handle, Crabbox, ACQUIRE_DEADLINE};

/// Warms up a Box for `req`; a failed warmup of a new Box releases what it made. `note` hears
/// what crabbox reports while it works.
pub(super) fn acquire(
    cb: &Crabbox,
    req: &AcquireReq,
    note: &mut dyn FnMut(&str),
) -> Result<Acquired, DriverError> {
    if super::pool::is_pool_provider(&req.provider) {
        return super::pool::acquire(cb, req);
    }
    cb.require_version()?;
    let provider = req.provider.as_str();
    let name = slug(req);
    let class = size_of(req.class.as_deref())?;
    let spec = cache::with_volumes(Crabbox::spec(ACQUIRE_DEADLINE), &req.cache)
        .args(["warmup", "--provider", provider])
        .args(["--slug", &name, "--class", class]);
    let handle = handle_of(provider, &name);
    let existed = cb.lease_dir(&handle)?.exists();
    let spec = cb.at_lease(spec, &handle)?;
    cb.forget_targets(&handle);
    match warm_up(cb, provider, &handle, &spec, req.cache.work_tree(), note) {
        Err(failed) if !existed => Err(cb.undo_acquire(&handle, failed)),
        acquired => acquired,
    }
}

/// Runs the warmup `spec` for `handle` and reads where its workdir is. A Box that keeps its work
/// tree works in `<workroot>/<work_tree>`, the same path in every Box of its repo. Otherwise it
/// is the ready line's `workdir`, else crabbox's own `<workroot>/<lease id>/<repo>`, where the
/// repo is the lease dir moor runs crabbox from, named after the Box. With none of them, the
/// acquire fails: moor never searches the Box for a directory by name. `note` hears what crabbox
/// reports while it works.
pub(super) fn warm_up(
    cb: &Crabbox,
    provider: &str,
    handle: &BoxHandle,
    spec: &ProcSpec,
    work_tree: Option<&str>,
    note: &mut dyn FnMut(&str),
) -> Result<Acquired, DriverError> {
    let mut watch = Watch::default();
    let out = cb
        .proc
        .run_watched(spec, &mut |line| watch.see(line, note))
        .map_err(|e| watch.explain(map_error(Op::Acquire, Some(provider), &e)))?;
    let warm = parse::parse_warmup(&out.stdout).ok_or_else(|| unreadable("acquire"))?;
    if let Some(root) = warm.workroot.as_deref().filter(|r| !r.starts_with('/')) {
        return Err(DriverError::Failed {
            summary: format!(
                "crabbox reported the relative work root {root:?}; it must resolve it against the login home"
            ),
        });
    }
    let derived = warm.workroot.as_deref().map(|root| {
        format!(
            "{}/{}/{}",
            root.trim_end_matches('/'),
            warm.id,
            split_handle(handle).1
        )
    });
    let kept = warm
        .workroot
        .as_deref()
        .zip(work_tree)
        .map(|(root, tree)| format!("{}/{tree}", root.trim_end_matches('/')));
    let workdir = [kept, warm.workdir, derived]
        .into_iter()
        .flatten()
        .find_map(|dir| BoxPath::restore(&dir).ok())
        .ok_or_else(|| DriverError::Failed {
            summary: "crabbox reported neither a workdir nor a workroot for the Box".to_owned(),
        })?;
    Ok(Acquired {
        handle: handle.clone(),
        workdir,
    })
}
