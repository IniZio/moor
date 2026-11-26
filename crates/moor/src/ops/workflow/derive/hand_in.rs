//! `reading-hand-in`: the Box reading an automation's prompt hands in phrases and effects. moor
//! checks them against the saved prompt and the config's `proxy.allow`, keeps the reading with
//! its gates in one transaction, and closes the Box. Each refusal says what to fix and is counted
//! on the draft; the third fails the draft, naming the last.

use std::fmt::Write as _;

use moor_core::config::parse_repo;
use moor_core::gate::Gates;
use moor_core::reading::{
    Context, ProxyAllow, Read, ReaderRun, Reading, ReadingId, ReadingOf, ReadingState,
};
use moor_core::request::{ReadingHandInReq, ReadingHandInValue};
use moor_core::seam::clock::Ts;
use moor_core::workflow::{PlanDraft, PromptHash, Workflow, READER_INSTRUCTIONS};
use moor_core::ErrorKind;

use super::close;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// The refusals a reader gets before its reading fails.
const MAX_REFUSALS: u32 = 3;
/// The agent moor runs in a reader's Box: data on the reading, never branched on.
const READER_HARNESS: &str = "claude-code";
/// Why a Box that reads no prompt may not hand in.
const NOT_A_READER: &str = "this Box is not reading an automation's prompt for moor; only the \
                            Box moor started to read one may hand in its reading";

/// A refusal the reader reads.
fn refused(why: impl Into<String>) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.into())
}

/// Checks the hand-in of `req.box_id` and keeps it, or says what to fix.
pub(crate) fn run(cx: &Cx, req: &ReadingHandInReq) -> Result<ReadingHandInValue, CommandError> {
    let store = cx.store()?;
    let Some(row) = store.box_row(req.box_id)? else {
        return Err(refused(format!("no Box {}", req.box_id.get())));
    };
    let Some((w, draft)) = reader_of(store, &row)? else {
        return Err(refused(NOT_A_READER));
    };
    let now = cx.clock().now();
    let reading = reading_for(store, &w, &row, (&draft, now))?;
    match reading.hand_in(&req.reading, &proxy_allow(&w)) {
        Ok(read) => {
            let reply = format!(
                "accepted: moor keeps {} phrases and {} effects. End your turn.",
                read.phrases().len(),
                read.effects().len()
            );
            let reading = Reading {
                state: ReadingState::Read(read),
                ..reading
            };
            if !store.hold_reader_read(&reading, &draft)? {
                return Err(refused(
                    "moor no longer waits for this reading: the prompt changed or the reading \
                     ended. End your turn.",
                ));
            }
            close(cx, store, row.name.as_str());
            Ok(ReadingHandInValue { reply })
        }
        Err(refusal) => Err(count(cx, store, (&w, &draft), &row, &refusal.to_string())),
    }
}

/// Keeps the gates an older reader left in `plan.json` as a reading with no phrases.
pub(super) fn hold_gates(
    cx: &Cx,
    store: &Store,
    (w, draft): (&Workflow, &PlanDraft),
    row: &BoxRow,
    gates: &Gates,
) -> Result<(), CommandError> {
    let reading = reading_for(store, w, row, (draft, cx.clock().now()))?;
    let reading = Reading {
        state: ReadingState::Read(Read::from_gates(&gates.doc())),
        ..reading
    };
    store.hold_reader_read(&reading, draft)?;
    Ok(())
}

/// Counts the refusal `why` on the draft, failing it and closing the Box at the last one; the
/// refusal the reader gets.
fn count(
    cx: &Cx,
    store: &Store,
    (w, draft): (&Workflow, &PlanDraft),
    row: &BoxRow,
    why: &str,
) -> CommandError {
    let counted = draft.refused_again();
    let n = counted.refused;
    if n < MAX_REFUSALS {
        let _ = store.move_plan_draft(w.id, draft, Some(&counted));
        return refused(format!(
            "{why}. Fix it and hand in again: refusal {n} of {MAX_REFUSALS}."
        ));
    }
    let failed = counted.failed(format!(
        "moor refused the reader's hand-in {MAX_REFUSALS} times; the last: {why}"
    ));
    if matches!(store.move_plan_draft(w.id, draft, Some(&failed)), Ok(true)) {
        close(cx, store, row.name.as_str());
    }
    refused(format!(
        "{why}. That was refusal {MAX_REFUSALS} of {MAX_REFUSALS}, so this reading failed. End \
         your turn."
    ))
}

/// The workflow whose prompt the Box `row` reads, with its draft; none when it reads none, or
/// moor no longer wants that reading.
fn reader_of(store: &Store, row: &BoxRow) -> Result<Option<(Workflow, PlanDraft)>, CommandError> {
    let found = store
        .plan_drafts()?
        .into_iter()
        .find(|(_, d)| d.failed.is_none() && d.box_name.as_deref() == Some(row.name.as_str()));
    let Some((id, draft)) = found else {
        return Ok(None);
    };
    let Some(w) = store.workflow_by_id(id)? else {
        return Ok(None);
    };
    let by_hand = w.gates.as_ref().is_some_and(|g| g.made_from().is_none());
    let wanted = draft.is_for(&w.prompt) && (!by_hand || draft.replace);
    Ok(wanted.then_some((w, draft)))
}

/// A reading of `w`'s saved prompt by the Box `row`, in flight since the draft asked for it.
fn reading_for(
    store: &Store,
    w: &Workflow,
    row: &BoxRow,
    (draft, now): (&PlanDraft, Ts),
) -> Result<Reading, CommandError> {
    let since = draft.since.map_or(now, Ts::from_millis);
    Ok(Reading {
        id: new_id(now)?,
        of: ReadingOf::Automation(w.id),
        parent: None,
        source: w.prompt.clone(),
        context: Context {
            source_hash: PromptHash::of(&w.prompt),
            repos: w.repos.clone(),
            rules: store.rules_pins(&w.repos)?,
            instructions: READER_INSTRUCTIONS,
        },
        made_by: w.owner,
        state: ReadingState::Reading {
            by: ReaderRun {
                box_name: row.name.clone(),
                harness: READER_HARNESS.to_owned(),
                model: None,
            },
            since,
        },
    })
}

/// The `proxy.allow` of `w`'s config; empty when it has none. A stored config parsed when saved.
fn proxy_allow(w: &Workflow) -> ProxyAllow {
    w.config
        .as_deref()
        .and_then(|text| parse_repo("the automation's config", text).ok())
        .map(|c| c.proxy.allow)
        .unwrap_or_default()
}

/// A new reading id: a version 7 UUID of `now` and random bits.
fn new_id(now: Ts) -> Result<ReadingId, CommandError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| CommandError::new(ErrorKind::Internal, e.to_string()))?;
    let ms = u64::try_from(now.as_millis()).unwrap_or(0).to_be_bytes();
    for (slot, b) in bytes.iter_mut().zip(ms.iter().skip(2)) {
        *slot = *b;
    }
    let [_, _, _, _, _, _, version, _, variant, ..] = &mut bytes;
    *version = (*version & 0x0f) | 0x70;
    *variant = (*variant & 0x3f) | 0x80;
    let mut text = String::with_capacity(36);
    for (at, b) in bytes.iter().enumerate() {
        if matches!(at, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{b:02x}");
    }
    ReadingId::parse(&text).map_err(|e| CommandError::new(ErrorKind::Internal, e.to_string()))
}
