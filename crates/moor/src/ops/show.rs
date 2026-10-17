//! `show`: reconcile, then report tasks, one task's detail and its events.
//! Each task's agent is read once per command; a named task reads only its own.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use moor_core::boxscript::denials;
use moor_core::decide::proof::ProofFile;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::view::{AttentionView, EventView, PaneView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq, TaskId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{
    agent_view, event_view, hide_unsupported_sleep, hide_unsupported_sleep_cached,
    task_view_with_run,
};
use crate::ops::common::{activity, autosleep, reconcile};
use crate::store::{Store, TaskRow};

/// Lines of the pane kept as the tail.
const PANE_LINES: usize = 40;

/// Minutes of sandbox log a denial listing covers.
const DENIAL_MINUTES: u32 = 30;

/// Wall-clock allowance for the denial listing.
const DENIALS_DEADLINE: Duration = Duration::from_secs(20);

/// Makes each denial log name unique within the process.
static DENIALS_SEQ: AtomicU64 = AtomicU64::new(0);

/// Stand-in for a proxy nonce in pane text.
const REDACTED: &str = "<nonce>";

/// Length of a base64url-encoded 32-byte nonce.
const NONCE_LEN: usize = 43;

/// `text` with the task's proxy nonce, and any `moor:<43 base64url chars>@` userinfo,
/// replaced by `<nonce>`. The proxy URL in the agent's environment carries the nonce.
pub(crate) fn redact_nonce(text: &str, nonce: Option<&[u8]>) -> String {
    const USER: &str = "moor:";
    let known = nonce
        .and_then(|n| std::str::from_utf8(n).ok())
        .filter(|n| !n.is_empty());
    let text = known.map_or_else(|| text.to_owned(), |n| text.replace(n, REDACTED));
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find(USER) {
        let (head, tail) = rest.split_at(at + USER.len());
        out.push_str(head);
        let seg = tail.as_bytes();
        let is_nonce = seg.get(NONCE_LEN) == Some(&b'@')
            && seg
                .iter()
                .take(NONCE_LEN)
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if is_nonce {
            out.push_str(REDACTED);
            // The first 43 bytes are ASCII, so the split is on a char boundary.
            rest = tail.split_at(NONCE_LEN).1;
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The pane section of a reading: its last lines and liveness; none without an observation.
/// The task's proxy nonce is redacted from the tail.
fn pane_view(
    reading: &activity::Reading,
    store: &Store,
    task: TaskId,
) -> Result<Option<PaneView>, CommandError> {
    let Some(text) = reading.pane_text.as_deref() else {
        return Ok(None);
    };
    let row = store.proxy(task)?;
    let skip = text.lines().count().saturating_sub(PANE_LINES);
    let tail = text.lines().skip(skip).collect::<Vec<_>>().join("\n");
    let tail = redact_nonce(&tail, row.as_ref().map(|r| r.nonce.expose()));
    Ok(Some(PaneView {
        tail,
        status: reading.pane.as_str().to_owned(),
    }))
}

/// Proof files in the task's Box; none, with a warning, when the Box cannot be read.
fn proof_files(cx: &Cx, row: &TaskRow) -> Option<Vec<ProofFile>> {
    let handle = row
        .boxed
        .as_ref()
        .filter(|b| b.state == BoxState::Ready)
        .and_then(|b| b.handle.as_ref())?;
    match crate::ops::proof::list(cx, handle, row.id, None) {
        Ok(files) => Some(files),
        Err(why) => {
            cx.warn(Warning {
                code: "proof_unavailable".to_owned(),
                message: format!("could not list proof files: {why}"),
            });
            None
        }
    }
}

/// Sandbox denials of a macOS Task: `live` are its own processes', `setup` those saved from the
/// setup time window.
struct SandboxDenials {
    /// From the Task's own sandbox processes.
    live: Vec<String>,
    /// Saved from the setup window; may include other processes.
    setup: Vec<String>,
}

/// Recent Seatbelt denials of a macOS Task, read from its Box in one exec. None for any other
/// Task, and when the Box cannot be read: `show` never fails on it. The text is untrusted, so
/// each line is stripped of control characters, capped and has the proxy nonce redacted.
fn sandbox_denials(cx: &Cx, row: &TaskRow) -> Option<SandboxDenials> {
    let handle = row
        .boxed
        .as_ref()
        .filter(|b| b.state == BoxState::Ready)
        .and_then(|b| b.handle.as_ref())?;
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let workdir = cx.driver().workdir(handle, worktree).ok()?;
    let layout = MacLayout::find(&workdir, &TaskKey::new(&row.name))?;
    let state = cx.state_dir().ok()?;
    let seq = DENIALS_SEQ.fetch_add(1, Ordering::Relaxed);
    let log = state.join(format!(
        "denials-{}-{}-{seq}.log",
        row.id.get(),
        std::process::id()
    ));
    let script = denials::script(layout.root.as_str(), workdir.as_str(), DENIAL_MINUTES);
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: Some(Output::Log(log.clone())),
        host_cwd: None,
        class: None,
    };
    let ran = cx.driver().exec(handle, &spec, Deadline(DENIALS_DEADLINE));
    let text = fs::read_to_string(&log);
    let _ = fs::remove_file(&log);
    match (ran, text) {
        (Ok(ExecOutcome::Exited { code: 0 }), Ok(text)) => {
            let proxy = cx.store().ok()?.proxy(row.id).ok()?;
            let nonce = proxy.as_ref().map(|r| r.nonce.expose());
            let tidy = |list: Vec<denials::Denial>| -> Vec<String> {
                list.iter()
                    .map(|d| denials::clean(&redact_nonce(&d.to_string(), nonce)))
                    .collect()
            };
            let split = denials::parse_split(&text);
            Some(SandboxDenials {
                live: tidy(split.live),
                setup: tidy(split.setup),
            })
        }
        _ => None,
    }
}

/// The task's open credential limits as views.
fn attention(store: &Store, task: TaskId) -> Result<Vec<AttentionView>, CommandError> {
    Ok(store
        .open_limits(task)?
        .iter()
        .map(AttentionView::from)
        .collect())
}

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let Some(name) = &req.task else {
        let mut tasks: Vec<TaskView> = Vec::new();
        let mut sleep_caps = HashMap::new();
        for mut row in store.open_tasks()? {
            let reading = activity::read(cx, store, &row, now)?;
            activity::settle(cx, store, &row, &reading)?;
            autosleep::run(cx, store, row.id);
            row = store.task(row.id)?.unwrap_or(row);
            let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
            let mut view = task_view_with_run(&row, in_flight.as_ref(), cx.config());
            view.agent = agent_view(&reading);
            view.attention = attention(store, row.id)?;
            hide_unsupported_sleep_cached(&mut view, cx, &mut sleep_caps);
            tasks.push(view);
        }
        return Ok(ShowView {
            tasks: Some(tasks),
            task: None,
            events: None,
            pane: None,
            reports: None,
            proofs: None,
            sandbox_denials: None,
            setup_denials: None,
        });
    };
    let row = store.latest_task_by_name(name)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no task {}", name.as_str()))
    })?;
    let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
    let reading = activity::read(cx, store, &row, now)?;
    let (agent, pane) = (agent_view(&reading), pane_view(&reading, store, row.id)?);
    let mut detail = task_view_with_run(&row, in_flight.as_ref(), cx.config());
    detail.agent = agent;
    detail.attention = attention(store, row.id)?;
    hide_unsupported_sleep(&mut detail, cx);
    let events = match req.events {
        Some(n) => {
            let last = usize::try_from(n).unwrap_or(usize::MAX);
            let rows = store.events(row.id, last)?;
            Some(rows.iter().map(event_view).collect::<Vec<EventView>>())
        }
        None => None,
    };
    let proofs = proof_files(cx, &row);
    let denied = sandbox_denials(cx, &row);
    Ok(ShowView {
        tasks: None,
        task: Some(detail),
        events,
        pane: pane.filter(|_| req.pane),
        reports: None,
        proofs,
        sandbox_denials: denied.as_ref().map(|d| d.live.clone()),
        setup_denials: denied.map(|d| d.setup),
    })
}

#[cfg(test)]
mod tests;
