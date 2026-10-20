//! `show`: reconcile, then report Boxes, one Box's detail and its events.
//! Each Box's agent is read once per command; a named Box reads only its own.

use std::collections::HashMap;

use moor_core::view::{ArtifactView, AttentionView, BoxView, EventView, PaneView, ShowView};
use moor_core::{BoxId, ErrorKind, ShowReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::artifacts::artifact_view;
use crate::ops::common::view::{
    agent_view, box_view_with_run, event_view, hide_unsupported_sleep,
    hide_unsupported_sleep_cached,
};
use crate::ops::common::{activity, autosleep, reconcile};
use crate::store::Store;

/// Lines of the pane kept as the tail.
const PANE_LINES: usize = 40;

/// Stand-in for a proxy nonce in pane text.
const REDACTED: &str = "<nonce>";

/// Length of a base64url-encoded 32-byte nonce.
const NONCE_LEN: usize = 43;

/// `text` with the Box's proxy nonce, and any `moor:<43 base64url chars>@` userinfo,
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
/// Proxy URL userinfo is redacted from the tail.
fn pane_view(reading: &activity::Reading) -> Option<PaneView> {
    let text = reading.pane_text.as_deref()?;
    let skip = text.lines().count().saturating_sub(PANE_LINES);
    let tail = text.lines().skip(skip).collect::<Vec<_>>().join("\n");
    let tail = redact_nonce(&tail, None);
    Some(PaneView {
        tail,
        status: reading.pane.as_str().to_owned(),
    })
}

/// The Box's open credential limits as views.
fn attention(store: &Store, box_id: BoxId) -> Result<Vec<AttentionView>, CommandError> {
    Ok(store
        .open_limits(box_id)?
        .iter()
        .map(AttentionView::from)
        .collect())
}

/// The latest version of each of the Box's artifacts as views.
fn artifacts_of(store: &Store, box_id: BoxId) -> Result<Vec<ArtifactView>, CommandError> {
    Ok(store
        .latest_artifacts(box_id)?
        .iter()
        .map(artifact_view)
        .collect())
}

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let Some(name) = &req.box_name else {
        let mut boxes: Vec<BoxView> = Vec::new();
        let mut sleep_caps = HashMap::new();
        for mut row in store.open_boxes()? {
            let reading = activity::read(cx, store, &row, now)?;
            activity::settle(cx, store, &row, &reading)?;
            autosleep::run(cx, store, row.id);
            row = store.box_row(row.id)?.unwrap_or(row);
            let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
            let mut view = box_view_with_run(&row, in_flight.as_ref(), cx.config());
            view.agent = agent_view(&reading);
            view.attention = attention(store, row.id)?;
            hide_unsupported_sleep_cached(&mut view, cx, &mut sleep_caps);
            boxes.push(view);
        }
        return Ok(ShowView {
            boxes: Some(boxes),
            boxed: None,
            events: None,
            pane: None,
            reports: None,
            artifacts: None,
        });
    };
    let row = store
        .latest_box_by_name(name)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {}", name.as_str())))?;
    let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
    let reading = activity::read(cx, store, &row, now)?;
    let (agent, pane) = (agent_view(&reading), pane_view(&reading));
    let mut detail = box_view_with_run(&row, in_flight.as_ref(), cx.config());
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
    let artifacts = if req.artifacts {
        Some(artifacts_of(store, row.id)?)
    } else {
        None
    };
    Ok(ShowView {
        boxes: None,
        boxed: Some(detail),
        events,
        pane: pane.filter(|_| req.pane),
        reports: None,
        artifacts,
    })
}

#[cfg(test)]
mod tests;
