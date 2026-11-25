//! The workflows pages' data: a person's workflows with their last run, one workflow's prompt
//! and plan read and replaced as them, its runs newest first, and one run's frozen plan joined
//! to its history. The controller decides who may read and who may change; this carries the
//! answer, tells a stale write from other refusals, and builds the lines the page shows, so the
//! page only places text.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowRunListReq};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::workflow::{
    RunRowView, WorkflowListValue, WorkflowRunListValue, WorkflowView,
};
use serde_json::{json, Value};

mod download;
mod file;
mod form;
mod now;
mod run;
mod save;
mod schedule;
mod verdict;

pub(super) use download::artifact;
pub(super) use form::{create, edit, remove, switch};
pub(super) use now::run_now;
pub(super) use run::{answer, run};
pub(super) use save::write;
pub(super) use schedule::preview;

use super::account::{failed, json};
use super::repos::answered;
use super::Session;
use crate::{tools, Gate};

/// What a workflow is read and written under: this, then its name.
const PREFIX: &str = "/account/api/workflows";

/// The largest plan write the page reads: the controller's own cap on a plan, with room for
/// the prompt and JSON escapes.
pub(super) const BODY_MAX: usize = 256 * 1024;

/// The most a check's output shows: the last 4 KiB the controller kept.
const TAIL_MAX: usize = 4096;

/// The most an artifact download holds in memory.
const DOWNLOAD_MAX: u64 = 32 * 1024 * 1024;

/// Where a route points.
pub(super) enum Route<'a> {
    /// All the person's workflows.
    List,
    /// One workflow: read it, or replace it.
    One(&'a str),
    /// One run of a workflow.
    Run(&'a str, i64),
    /// The answer to the approve step one run waits at.
    Answer(&'a str, i64),
    /// A run of the workflow fired now.
    RunNow(&'a str),
    /// One artifact's bytes.
    Artifact,
    /// A workflow's repos, prompt, schedule and timeout, replaced from the form.
    Fields(&'a str),
    /// Whether a workflow fires on its schedule.
    Enabled(&'a str),
    /// The words and next firings of a schedule, before it is saved.
    Schedule,
}

/// The route `path` names, when it is one.
pub(super) fn route(path: &str) -> Option<Route<'_>> {
    if path == "/account/api/artifact" {
        return Some(Route::Artifact);
    }
    if path == "/account/api/schedule" {
        return Some(Route::Schedule);
    }
    let rest = path.strip_prefix(PREFIX)?;
    if rest.is_empty() {
        return Some(Route::List);
    }
    let parts: Vec<&str> = rest.strip_prefix('/')?.split('/').collect();
    match parts.as_slice() {
        [name] if !name.is_empty() => Some(Route::One(name)),
        [name, "runs", id] => Some(Route::Run(name, id.parse().ok()?)),
        [name, "runs", id, "answer"] => Some(Route::Answer(name, id.parse().ok()?)),
        [name, "run"] => Some(Route::RunNow(name)),
        [name, "fields"] => Some(Route::Fields(name)),
        [name, "enabled"] => Some(Route::Enabled(name)),
        _ => None,
    }
}

/// `name` decoded from a path segment.
fn decode(name: &str) -> String {
    let wrapped = format!("v={}", name.replace('+', "%2B").replace('&', "%26"));
    oauth2::url::form_urlencoded::parse(wrapped.as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Answers the person's workflows, each with its latest run.
pub(super) fn list(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    match workflows(gate, session) {
        Ok(all) => {
            let rows: Vec<Value> = all.iter().map(summary).collect();
            json(stream, "200 OK", &json!({"workflows": rows}));
        }
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Missing) => not_found(stream),
        Err(Failure::Other(why)) => failed(stream, "workflow list", &why),
    }
}

/// Why a read of the controller gave no value.
enum Failure {
    /// The controller said no.
    Reply(Box<ControllerResponse>),
    /// There is no such workflow of the person's: it does not exist or is someone else's, which
    /// the page is never told apart.
    Missing,
    /// It could not be asked, or its answer was unreadable.
    Other(String),
}

impl Failure {
    /// Reads a refusal of a read or write of one named workflow as `Missing`: every workflow
    /// the controller shows a person is theirs, so a refusal is no such workflow of theirs.
    fn own<T>(result: Result<T, Self>) -> Result<T, Self> {
        match result {
            Err(Self::Reply(reply)) if reply.class == ExitClass::Refused => Err(Self::Missing),
            other => other,
        }
    }
}

/// Answers `404` for a workflow that is not the person's.
fn not_found(stream: &mut TcpStream) {
    json(
        stream,
        "404 Not Found",
        &json!({"error": "no such workflow"}),
    );
}

/// The value of `request`, asked as the session's person.
fn value_of<T: serde::de::DeserializeOwned>(
    gate: &Gate,
    session: &Session,
    request: ControllerRequest,
) -> Result<T, Failure> {
    let reply =
        tools::ask_as(&gate.controller, &session.person, request).map_err(Failure::Other)?;
    if reply.class != ExitClass::Ok {
        return Err(Failure::Reply(Box::new(reply)));
    }
    let value = reply.value.unwrap_or_default();
    serde_json::from_value(value).map_err(|e| Failure::Other(format!("unreadable answer: {e}")))
}

/// Every workflow of the session's person.
fn workflows(gate: &Gate, session: &Session) -> Result<Vec<WorkflowView>, Failure> {
    let list = ControllerRequest::WorkflowList(moor_core::request::AsReq { identity: None });
    value_of::<WorkflowListValue>(gate, session, list).map(|v| v.workflows)
}

/// Every workflow of the session's person, or none when they could not be read.
pub(super) fn listed(gate: &Gate, session: &Session) -> Option<Vec<WorkflowView>> {
    workflows(gate, session).ok()
}

/// Whether the run ended badly: it failed, timed out, was refused or left unfinished.
pub(super) fn went_wrong(run: &moor_core::view::workflow::LastRunView) -> bool {
    status(&run.phase, &run.outcome) == "failure"
}

/// The run's status, the way an Actions list shows it: one word for the icon.
fn status(phase: &str, outcome: &str) -> &'static str {
    match (phase, outcome) {
        ("queued", _) => "queued",
        ("running", _) => "running",
        ("waiting", _) => "waiting",
        (_, "finished") => "success",
        (_, "skipped_overlap" | "skipped_fork") => "skipped",
        (_, "declined") => "stopped",
        _ => "failure",
    }
}

/// One line of a workflow list.
fn summary(w: &WorkflowView) -> Value {
    let last = w.last_run.as_ref();
    json!({
        "name": w.name,
        "repos": w.repos,
        "cron": w.cron,
        "words": schedule::summary_words(w.cron.as_deref(), w.on_pull_request),
        "timezone": w.timezone,
        "enabled": w.enabled,
        "on_pull_request": w.on_pull_request,
        "next_due": w.next_due,
        "next_text": w
            .next_due
            .zip(w.timezone.as_deref())
            .and_then(|(ms, zone)| schedule::at_in_zone(ms, zone)),
        "has_plan": w.plan.is_some(),
        "plan_stale": w.plan_stale,
        "last": last.map(|r| json!({
            "status": status(&r.phase, &r.outcome),
            "label": verdict::label_of(r),
            "verdict": verdict::named(r),
            "due_at": r.due_at,
        })),
    })
}

/// What a run shows beside its icon: its outcome once it is done, otherwise its phase and step.
fn label(phase: &str, outcome: &str, step: Option<&str>) -> String {
    let at = step.map(|s| format!(" at {s}")).unwrap_or_default();
    match phase {
        "done" => format!("{}{at}", outcome.replace('_', " ")),
        "waiting" => format!("waiting on you{at}"),
        other => format!("{other}{at}"),
    }
}

/// When a run started and ended, from its history, in milliseconds.
fn span(run: &moor_core::view::workflow::LastRunView) -> (Option<i64>, Option<i64>) {
    let started = run.history.first().map(|l| l.at);
    let ended = run
        .history
        .iter()
        .rev()
        .find(|l| {
            matches!(
                l.entry.get("kind").and_then(Value::as_str),
                Some("ended" | "refused" | "skipped" | "skipped_fork")
            )
        })
        .map(|l| l.at);
    (started, ended)
}

/// One run as a list row.
fn run_row(row: &RunRowView) -> Value {
    let run = &row.run;
    let (started, ended) = span(run);
    let duration = started.zip(ended).map(|(s, e)| (e - s).max(0));
    json!({
        "id": row.id,
        "due_at": run.due_at,
        "trigger": run.trigger,
        "pull": run.pull,
        "started_at": started,
        "duration_ms": duration,
        "status": status(&run.phase, &run.outcome),
        "label": verdict::label_of(run),
        "verdict": verdict::named(run),
        "result": run.result,
        "detail": run.detail,
    })
}

/// Answers one workflow: its prompt, its plan as text, and its runs.
pub(super) fn read(stream: &mut TcpStream, gate: &Gate, session: &Session, name: &str) {
    let name = decode(name);
    match detail(gate, session, &name) {
        Ok(view) => json(stream, "200 OK", &view),
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Missing) => not_found(stream),
        Err(Failure::Other(why)) => failed(stream, "workflow read", &why),
    }
}

/// The workflow `name` and its latest runs, as the page reads them.
fn detail(gate: &Gate, session: &Session, name: &str) -> Result<Value, Failure> {
    let all = Failure::own(workflows(gate, session))?;
    let Some(w) = all.into_iter().find(|w| w.name == name) else {
        return Err(Failure::Missing);
    };
    let listed = Failure::own(value_of::<WorkflowRunListValue>(
        gate,
        session,
        ControllerRequest::WorkflowRunList(WorkflowRunListReq {
            identity: None,
            workflow: name.to_owned(),
            run: None,
            limit: None,
        }),
    ))?;
    let runs: Vec<Value> = listed.runs.iter().map(run_row).collect();
    let edited = w
        .plan
        .as_ref()
        .and_then(|p| p.get("edited"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let plan_text = w
        .plan
        .as_ref()
        .and_then(|p| serde_json::to_string_pretty(p).ok());
    let mut view = summary(&w);
    if let Some(map) = view.as_object_mut() {
        map.insert("config".to_owned(), json!(w.config));
        map.insert("may".to_owned(), file::may(w.config.as_deref()));
        map.insert("run_as".to_owned(), json!(session.person.email.as_str()));
        map.insert("timeout_minutes".to_owned(), json!(w.timeout_minutes));
        map.insert("prompt".to_owned(), json!(w.prompt));
        map.insert("plan".to_owned(), json!(plan_text));
        map.insert("gates".to_owned(), json!(w.gates));
        map.insert("outcomes".to_owned(), json!(w.outcomes));
        map.insert("plan_edited".to_owned(), json!(edited));
        map.insert("plan_stale".to_owned(), json!(w.plan_stale));
        map.insert("plan_making".to_owned(), json!(w.plan_making));
        map.insert("revision".to_owned(), json!(w.revision));
        map.insert("runs".to_owned(), json!(runs));
    }
    Ok(view)
}
