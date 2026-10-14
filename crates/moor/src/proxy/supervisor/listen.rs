//! The nono proxy of one supervised Task, with its capture and request log.

use std::sync::Arc;

use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{Secret, TaskId};

use super::run::say;
use super::Deps;
use crate::proxy::nono::audit::{apply, TaskSink};
use crate::proxy::nono::capture::{sources_for, MoorCapture};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::proxy::ProxyLog;

/// A started proxy and what it reports into.
pub(super) struct Live {
    /// The nono proxy.
    pub(super) proxy: NonoProxy,
    /// Credential cache the proxy and the audit drain share.
    pub(super) capture: Arc<MoorCapture>,
    /// The Task's request log.
    pub(super) log: ProxyLog,
}

/// Loads the Task CA, opens the log and starts the proxy; the caller still holds the lease on failure.
pub(super) fn listen(
    deps: &Deps<'_>,
    task: TaskId,
    nonce: &Secret,
    spec: &ProxySpec,
) -> Result<Live, String> {
    let ca = ca::load_or_create(deps.state_dir, task)
        .map_err(|err| format!("cannot load the proxy CA: {err}"))?;
    let sources = sources_for(
        &spec.rules,
        Arc::clone(&deps.creds),
        Arc::clone(&deps.forge),
        spec.task_repo.clone(),
    );
    let capture = Arc::new(MoorCapture::new(sources, Arc::clone(&deps.clock)));
    let cfg = build_config(&NonoSpec {
        rules: &spec.rules,
        allow: &spec.allow,
        nonce,
        ca: &ca,
        ca_dir: &ca::ca_dir(deps.state_dir, task),
        upstream_ca: deps.upstream_ca.as_deref(),
    });
    let log = ProxyLog::open(deps.state_dir, task, Arc::clone(&deps.clock))
        .map_err(|err| format!("cannot open the proxy log: {err}"))?;
    let proxy = NonoProxy::start(cfg, Arc::clone(&capture))
        .map_err(|err| format!("cannot start the proxy: {err}"))?;
    Ok(Live {
        proxy,
        capture,
        log,
    })
}

impl Live {
    /// Moves the audit events recorded since the last call into the log and the store.
    pub(super) fn drain(&self, deps: &Deps<'_>, task: TaskId) {
        let sink = TaskSink {
            log: &self.log,
            store: deps.store,
            task,
        };
        apply(&self.proxy.drain_audit(), &self.capture, &sink);
    }

    /// Removes the Task CA directory, which also holds the spec.
    pub(super) fn forget(deps: &Deps<'_>, task: TaskId) {
        if let Err(err) = ca::remove(deps.state_dir, task) {
            say(
                &deps.stderr,
                format_args!("cannot remove the proxy CA: {err}"),
            );
        }
    }
}
