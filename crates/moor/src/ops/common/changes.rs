//! What a Box changed in git, read from its work trees: the receipt recorded after each turn,
//! and the changes a person reads now. Neither wakes a Box nor records a run.

use std::time::Duration;

use moor_core::changes::{
    capped, parse_numstat, patch, snapshot, total_numstat, turn_numstat, RepoChanges, RepoReceipt,
    Tally, Tree, PATCH_BYTES, RECEIPT_FILES,
};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::AccessEvent;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{BoxStep, ExecOutcome, Place, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, Warning};

use super::provision::recorded_workdir;
use super::reports::target;
use crate::cx::Cx;
use crate::store::{BoxRow, Store};

/// Wall-clock allowance for each script.
const WITHIN: Duration = Duration::from_secs(20);

/// The scratch name of the receipt's snapshot.
const RECEIPT: &str = "receipt";

/// The work tree of each repo of `row`, first one first: the others sit beside the first.
pub(crate) fn trees(row: &BoxRow, workdir: &BoxPath) -> Vec<Tree> {
    let Some(repo) = row.repo.as_ref() else {
        return Vec::new();
    };
    let root = workdir.as_str().trim_end_matches('/');
    let parent = root.rsplit_once('/').map_or("", |(p, _)| p);
    let first = Tree {
        repo: repo.repo.to_string(),
        dir: root.to_owned(),
        base: row.base.clone(),
    };
    let others = repo.others.iter().map(|o| Tree {
        repo: o.repo.to_string(),
        dir: format!("{parent}/{}", o.repo.name),
        base: None,
    });
    std::iter::once(first).chain(others).collect()
}

/// The handle and work trees of a Box whose machine is up; none otherwise.
fn place(cx: &Cx, row: &BoxRow) -> Option<(BoxHandle, Vec<Tree>)> {
    let (b, handle) = target(row).filter(|_| !row.asleep())?;
    let workdir = match b.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => cx.driver().workdir(handle).ok()?,
    };
    Some((handle.clone(), trees(row, &workdir)))
}

/// Runs `scripts` in order in the Box's work place, which holds every repo's work tree: the agent
/// place sees only the first. What each printed, none for one that did not run.
fn run(cx: &Cx, handle: &BoxHandle, scripts: Vec<String>) -> Vec<Option<String>> {
    let count = scripts.len();
    let steps: Vec<BoxStep> = scripts.into_iter().map(BoxStep::Run).collect();
    let mut done = cx
        .driver()
        .batch(handle, &[], Place::Work, &steps, Deadline(WITHIN))
        .into_iter()
        .map(|d| match d {
            Ok(StepDone::Ran {
                outcome: ExecOutcome::Exited { code: 0 },
                stdout,
            }) => Some(stdout),
            _ => None,
        })
        .collect::<Vec<_>>();
    done.resize(count, None);
    done
}

/// Why a snapshot step's output says the repo could not be read; none when it could.
fn problem(said: Option<&String>) -> Option<String> {
    match said.map(|s| s.trim()) {
        Some("ok") => None,
        Some("") | None => Some("the Box did not answer in time".to_owned()),
        Some(why) => Some(why.to_owned()),
    }
}

/// Records what the turns since the last receipt changed in each repo of `row`, as a
/// `turn_changes` event. The caller holds the Box's turn. A failure only warns.
pub(crate) fn note_turn(cx: &Cx, store: &Store, row: &BoxRow) {
    let Some((turn, ended)) = turn_to_note(store, row) else {
        return;
    };
    let Some((handle, trees)) = place(cx, row) else {
        return;
    };
    let scripts = trees
        .iter()
        .flat_map(|t| {
            [
                snapshot(t, RECEIPT),
                turn_numstat(t, RECEIPT),
                total_numstat(t, RECEIPT, true),
            ]
        })
        .collect();
    let said = run(cx, &handle, scripts);
    let repos: Vec<RepoReceipt> = trees
        .iter()
        .zip(said.chunks(3))
        .filter_map(|(t, out)| {
            let [snap, turn, total] = out else {
                return None;
            };
            if problem(snap.as_ref()).is_some() {
                return None;
            }
            let mut files = parse_numstat(total.as_deref()?);
            let total = Tally::of(&files);
            files.truncate(RECEIPT_FILES);
            Some(RepoReceipt {
                repo: t.repo.clone(),
                turn: Tally::of(&parse_numstat(turn.as_deref()?)),
                total,
                files,
            })
        })
        .collect();
    let changed = repos.iter().any(|r| r.turn.files > 0);
    if repos.is_empty() || !(ended || changed) {
        return;
    }
    let event = ExtraEvent::Access(AccessEvent::TurnChanges { turn, repos });
    let recorded = store.apply(Decision::new(row.id).event(event));
    if let Err(e) = recorded {
        cx.warn(Warning {
            code: "turn_changes_failed".to_owned(),
            message: format!("the turn's changes were not recorded: {e}"),
        });
    }
}

/// The number of the turn a receipt would record, and whether a `turn_done` came since the last
/// receipt. Without one, as for a turn typed into the terminal, only a change is worth a receipt.
fn turn_to_note(store: &Store, row: &BoxRow) -> Option<(u64, bool)> {
    let done = store.count_events_of(row.id, "turn_done").ok()?;
    let last = store.newest_events_of(row.id, "turn_changes", 1).ok()?;
    let noted = last
        .first()
        .and_then(|e| e.payload.get("turn")?.as_u64())
        .unwrap_or(0);
    Some((done.max(noted + 1), done > noted))
}

/// Each repo of `row` as its work tree holds it now, with its patch; none when the Box's machine
/// is not up.
pub(crate) fn read_now(cx: &Cx, row: &BoxRow) -> Option<Vec<RepoChanges>> {
    read(cx, row, PATCH_BYTES)
}

/// Keeps the diff of `row` as a `diff_kept` event, so the Changes tab can show it while the Box
/// sleeps or after it closes. The patches of all repos share one budget of [`PATCH_BYTES`]; one
/// that does not fit is cut. A diff the same as the last one kept, or a Box that cannot be read
/// now, records nothing. The caller holds the Box's turn. A failure only warns.
pub(crate) fn keep_diff(cx: &Cx, store: &Store, row: &BoxRow) {
    if row.repo.is_none() {
        return;
    }
    let Some(mut repos) = read(cx, row, PATCH_BYTES) else {
        return;
    };
    repos.retain(|r| r.problem.is_none());
    if repos.is_empty() {
        return;
    }
    let mut room = PATCH_BYTES;
    for r in &mut repos {
        let (patch, cut) = capped(r.patch.as_deref().unwrap_or_default(), room);
        room = room.saturating_sub(patch.len());
        r.cut |= cut;
        r.patch = Some(patch);
    }
    if kept(store, row).is_some_and(|last| last == repos) {
        return;
    }
    let recorded = store
        .apply(Decision::new(row.id).event(ExtraEvent::Access(AccessEvent::DiffKept { repos })));
    if let Err(e) = recorded {
        cx.warn(Warning {
            code: "diff_kept_failed".to_owned(),
            message: format!("the Box's diff was not kept: {e}"),
        });
    }
}

/// The repos of the newest diff kept for `row`, and when it was kept.
pub(crate) fn kept_at(store: &Store, row: &BoxRow) -> Option<(Vec<RepoChanges>, Ts)> {
    let last = store.newest_events_of(row.id, "diff_kept", 1).ok()?.pop()?;
    let repos = serde_json::from_value(last.payload.get("repos")?.clone()).ok()?;
    Some((repos, last.ts))
}

/// The repos of the newest diff kept for `row`.
fn kept(store: &Store, row: &BoxRow) -> Option<Vec<RepoChanges>> {
    kept_at(store, row).map(|(repos, _)| repos)
}

/// [`read_now`] with each repo's patch cut at `cap`.
fn read(cx: &Cx, row: &BoxRow, cap: usize) -> Option<Vec<RepoChanges>> {
    let (handle, trees) = place(cx, row)?;
    let tag = format!("view{}", cx.clock().now().as_millis());
    let scripts = trees
        .iter()
        .flat_map(|t| {
            [
                snapshot(t, &tag),
                total_numstat(t, &tag, false),
                patch(t, &tag, cap),
            ]
        })
        .collect();
    let said = run(cx, &handle, scripts);
    let repos = trees
        .iter()
        .zip(said.chunks(3))
        .map(|(t, out)| {
            let part = |i: usize| out.get(i).and_then(Option::as_deref).unwrap_or_default();
            let files = parse_numstat(part(1));
            let (patch, cut) = capped(part(2), cap);
            RepoChanges {
                repo: t.repo.clone(),
                total: Tally::of(&files),
                files,
                patch: Some(patch),
                cut,
                problem: problem(out.first().and_then(Option::as_ref)).or_else(|| {
                    out.iter()
                        .any(Option::is_none)
                        .then(|| "the Box did not answer in time".to_owned())
                }),
            }
        })
        .collect();
    Some(repos)
}
