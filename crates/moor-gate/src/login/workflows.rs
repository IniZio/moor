//! The workflows pages' data: a person's workflows with their last run, one workflow's prompt
//! and plan read and replaced as them, its runs newest first, and one run's frozen plan joined
//! to its history. The controller decides who may read and who may change; this carries the
//! answer, tells a stale write from other refusals, and builds the lines the page shows, so the
//! page only places text.

use std::net::TcpStream;

use moor_core::request::{
    Request as ControllerRequest, WorkflowAddReq, WorkflowAnswerReq, WorkflowEnableReq,
    WorkflowRunListReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::workflow::{
    RunRowView, WorkflowListValue, WorkflowRunListValue, WorkflowView,
};
use moor_core::workflow::Answer;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::account::{failed, json};
use super::repos::answered;
use super::{check_csrf, Session};
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
    /// One artifact's bytes.
    Artifact,
}

/// The route `path` names, when it is one.
pub(super) fn route(path: &str) -> Option<Route<'_>> {
    if path == "/account/api/artifact" {
        return Some(Route::Artifact);
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
        _ => None,
    }
}

/// The body of a workflow write.
#[derive(Deserialize)]
struct Write {
    /// What each Box's agent is asked to do.
    prompt: String,
    /// The plan as JSON text; blank for none.
    plan: String,
    /// The revision the page read.
    revision: u64,
    /// Yes when the page asked the person and they agreed that a stored plan goes.
    #[serde(default)]
    clear_plan: bool,
}

/// The body of an answer.
#[derive(Deserialize)]
struct AnswerBody {
    /// `yes` or `no`.
    answer: String,
    /// What the owner adds for the agent.
    #[serde(default)]
    note: Option<String>,
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
        Err(Failure::Other(why)) => failed(stream, "workflow list", &why),
    }
}

/// Why a read of the controller gave no value.
enum Failure {
    /// The controller said no.
    Reply(ControllerResponse),
    /// It could not be asked, or its answer was unreadable.
    Other(String),
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
        return Err(Failure::Reply(reply));
    }
    let value = reply.value.unwrap_or_default();
    serde_json::from_value(value).map_err(|e| Failure::Other(format!("unreadable answer: {e}")))
}

/// Every workflow of the session's person.
fn workflows(gate: &Gate, session: &Session) -> Result<Vec<WorkflowView>, Failure> {
    let list = ControllerRequest::WorkflowList(moor_core::request::AsReq { identity: None });
    value_of::<WorkflowListValue>(gate, session, list).map(|v| v.workflows)
}

/// The run's status, the way an Actions list shows it: one word for the icon.
fn status(phase: &str, outcome: &str) -> &'static str {
    match (phase, outcome) {
        ("queued", _) => "queued",
        ("running", _) => "running",
        ("waiting", _) => "waiting",
        (_, "finished") => "success",
        (_, "skipped_overlap") => "skipped",
        (_, "declined") => "stopped",
        _ => "failure",
    }
}

/// One line of a workflow list.
fn summary(w: &WorkflowView) -> Value {
    let last = w.last_runs.iter().max_by_key(|r| r.due_at);
    json!({
        "name": w.name,
        "repos": w.repos,
        "cron": w.cron,
        "timezone": w.timezone,
        "enabled": w.enabled,
        "disabled_reason": w.disabled_reason,
        "next_due": w.next_due,
        "has_plan": w.plan.is_some(),
        "plan_stale": w.plan_stale,
        "last": last.map(|r| json!({
            "repo": r.repo,
            "status": status(&r.phase, &r.outcome),
            "label": label(&r.phase, &r.outcome, r.step.as_deref()),
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
                Some("ended" | "refused" | "skipped")
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
        "repo": run.repo,
        "due_at": run.due_at,
        "started_at": started,
        "duration_ms": duration,
        "status": status(&run.phase, &run.outcome),
        "label": label(&run.phase, &run.outcome, run.step.as_deref()),
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
        Err(Failure::Other(why)) => failed(stream, "workflow read", &why),
    }
}

/// The workflow `name` and its latest runs, as the page reads them.
fn detail(gate: &Gate, session: &Session, name: &str) -> Result<Value, Failure> {
    let all = workflows(gate, session)?;
    let Some(w) = all.into_iter().find(|w| w.name == name) else {
        return Err(Failure::Other(format!("no workflow named {name}")));
    };
    let listed = value_of::<WorkflowRunListValue>(
        gate,
        session,
        ControllerRequest::WorkflowRunList(WorkflowRunListReq {
            identity: None,
            workflow: name.to_owned(),
            run: None,
            limit: None,
        }),
    )?;
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
        map.insert("also".to_owned(), json!(w.also));
        map.insert("config".to_owned(), json!(w.config));
        map.insert("timeout_minutes".to_owned(), json!(w.timeout_minutes));
        map.insert("prompt".to_owned(), json!(w.prompt));
        map.insert("plan".to_owned(), json!(plan_text));
        map.insert("plan_edited".to_owned(), json!(edited));
        map.insert("revision".to_owned(), json!(w.revision));
        map.insert("runs".to_owned(), json!(runs));
    }
    Ok(view)
}

/// Replaces the prompt and plan of workflow `name` as the session's person, when the request
/// carries the CSRF token. A write at a revision that is no longer the stored one answers `409`.
pub(super) fn write(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let Some(write) = body.and_then(|b| serde_json::from_slice::<Write>(b).ok()) else {
        let error = json!({"error": "send the prompt, the plan and the revision you read"});
        return json(stream, "400 Bad Request", &error);
    };
    let name = decode(name);
    match save(gate, session, &name, &write) {
        Ok(()) => read(stream, gate, session, &name),
        Err(Saved::Conflict(now)) => changed(stream, now),
        Err(Saved::Bad(why)) => json(stream, "400 Bad Request", &why),
        Err(Saved::Refused(reply)) => answered(stream, &reply),
        Err(Saved::Other(why)) => failed(stream, "workflow write", &why),
    }
}

/// Why a save did not happen.
enum Saved {
    /// The workflow moved on; the revision it is at, none when it is gone.
    Conflict(Option<u64>),
    /// The write is wrong: a body for the page to show.
    Bad(Value),
    /// The controller refused.
    Refused(ControllerResponse),
    /// The controller could not be asked.
    Other(String),
}

impl From<Failure> for Saved {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Reply(r) => Self::Refused(r),
            Failure::Other(why) => Self::Other(why),
        }
    }
}

/// A plan with `edited` and nothing else dropped, for comparing a person's change to the
/// stored plan.
fn without_edited(plan: &Value) -> Value {
    let mut plan = plan.clone();
    if let Some(map) = plan.as_object_mut() {
        map.remove("edited");
    }
    plan
}

/// Stores the written prompt and plan on top of what the workflow already is.
fn save(gate: &Gate, session: &Session, name: &str, write: &Write) -> Result<(), Saved> {
    let found = workflows(gate, session)?
        .into_iter()
        .find(|w| w.name == name);
    let Some(stored) = found else {
        return Err(Saved::Conflict(None));
    };
    if stored.revision != write.revision {
        return Err(Saved::Conflict(Some(stored.revision)));
    }
    let plan = plan_to_store(&stored, write)?;
    let add = ControllerRequest::WorkflowAdd(WorkflowAddReq {
        identity: None,
        name: stored.name.clone(),
        cron: stored.cron.clone(),
        timezone: stored.timezone.clone(),
        repos: stored.repos.clone(),
        also: stored.also.clone(),
        config: stored.config.clone(),
        prompt: write.prompt.clone(),
        timeout_minutes: Some(stored.timeout_minutes),
        plan,
        expected_revision: Some(write.revision),
        origin_key: None,
    });
    let reply = tools::ask_as(&gate.controller, &session.person, add).map_err(Saved::Other)?;
    if reply.class == ExitClass::Refused {
        let now = workflows(gate, session)?
            .into_iter()
            .find(|w| w.name == name);
        match now {
            None => return Err(Saved::Conflict(None)),
            Some(now) if now.revision != write.revision => {
                return Err(Saved::Conflict(Some(now.revision)));
            }
            Some(_) => {}
        }
    }
    if reply.class == ExitClass::Usage {
        let why = reply
            .error
            .as_ref()
            .map_or("the controller refused the plan", |e| e.message.as_str());
        return Err(Saved::Bad(json!({"error": why})));
    }
    if reply.class != ExitClass::Ok {
        return Err(Saved::Refused(reply));
    }
    if !stored.enabled {
        // A replace enables a workflow; a person who only edited its plan did not ask for that.
        let off = ControllerRequest::WorkflowSetEnabled(WorkflowEnableReq {
            identity: None,
            name: stored.name,
            enabled: false,
        });
        tools::ask_as(&gate.controller, &session.person, off).map_err(Saved::Other)?;
    }
    Ok(())
}

/// The plan text to store for `write`: none when the page asked for none, and it said it
/// knows a stored plan goes; otherwise the written JSON, marked `edited` when it differs from
/// the stored plan.
fn plan_to_store(stored: &WorkflowView, write: &Write) -> Result<Option<String>, Saved> {
    let text = write.plan.trim();
    if text.is_empty() {
        if stored.plan.is_some() && !write.clear_plan {
            let prompt_changed = stored.prompt != write.prompt;
            let why = if prompt_changed {
                "This changes the prompt and leaves the plan empty, so the plan would be \
                 cleared and every run would deliver the prompt in one step."
            } else {
                "The plan is empty, so it would be cleared and every run would deliver the \
                 prompt in one step."
            };
            return Err(Saved::Bad(json!({"error": why, "code": "clears_plan"})));
        }
        return Ok(None);
    }
    let mut plan: Value = serde_json::from_str(text)
        .map_err(|e| Saved::Bad(json!({"error": format!("the plan is not JSON: {e}")})))?;
    let Some(map) = plan.as_object_mut() else {
        return Err(Saved::Bad(
            json!({"error": "the plan must be an object with a `steps` list"}),
        ));
    };
    let same = stored
        .plan
        .as_ref()
        .is_some_and(|p| without_edited(p) == without_edited(&Value::Object(map.clone())));
    let was_edited = stored
        .plan
        .as_ref()
        .and_then(|p| p.get("edited"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    map.insert("edited".to_owned(), json!(!same || was_edited));
    Ok(Some(plan.to_string()))
}

/// Answers `409`: the workflow is no longer at the revision the page wrote against.
fn changed(stream: &mut TcpStream, now: Option<u64>) {
    let error = now.map_or_else(
        || "This workflow was removed. Reload to see what is left.".to_owned(),
        |_| {
            "Someone changed this workflow after you read it. Reload to see it before you save."
                .to_owned()
        },
    );
    json(
        stream,
        "409 Conflict",
        &json!({"error": error, "current": {"revision": now}}),
    );
}

/// Answers one run: the frozen plan as steps, each with its history, check output and evidence.
pub(super) fn run(stream: &mut TcpStream, gate: &Gate, session: &Session, name: &str, id: i64) {
    let name = decode(name);
    match one_run(gate, session, &name, id) {
        Ok(Some(view)) => json(stream, "200 OK", &view),
        Ok(None) => json(stream, "404 Not Found", &json!({"error": "no such run"})),
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Other(why)) => failed(stream, "run read", &why),
    }
}

/// The run `id` of workflow `name`, joined to its plan.
fn one_run_row(
    gate: &Gate,
    session: &Session,
    name: &str,
    id: i64,
) -> Result<Option<RunRowView>, Failure> {
    let listed = value_of::<WorkflowRunListValue>(
        gate,
        session,
        ControllerRequest::WorkflowRunList(WorkflowRunListReq {
            identity: None,
            workflow: name.to_owned(),
            run: Some(id),
            limit: Some(1),
        }),
    )?;
    Ok(listed.runs.into_iter().find(|r| r.id == id))
}

/// The run as the run page reads it.
fn one_run(gate: &Gate, session: &Session, name: &str, id: i64) -> Result<Option<Value>, Failure> {
    Ok(one_run_row(gate, session, name, id)?.map(|row| run_view(name, &row)))
}

/// `value`'s string field `key`.
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The last `TAIL_MAX` bytes of `tail`, cut at a character.
fn clipped(tail: &str) -> &str {
    if tail.len() <= TAIL_MAX {
        return tail;
    }
    let mut from = tail.len() - TAIL_MAX;
    while !tail.is_char_boundary(from) {
        from += 1;
    }
    &tail[from..]
}

/// The href that downloads artifact `name` of Box `r#box`.
fn artifact_href(r#box: &str, name: &str) -> String {
    let query = oauth2::url::form_urlencoded::Serializer::new(String::new())
        .append_pair("box", r#box)
        .append_pair("name", name)
        .finish();
    format!("/account/api/artifact?{query}")
}

/// One history line as the page shows it: its kind, when, and a sentence.
fn line_of(at: i64, entry: &Value, r#box: Option<&str>) -> Value {
    let kind = text(entry, "kind").unwrap_or("unknown");
    let try_n = entry.get("try_n").and_then(Value::as_u64);
    let tries = try_n.map(|n| format!(" (try {n})")).unwrap_or_default();
    let mut evidence = Vec::new();
    let sentence = match kind {
        "queued" => "Queued; its Box is starting".to_owned(),
        "entered" => format!("Entered{tries}"),
        "sent" => format!("Instructions sent to the agent{tries}"),
        "reported" => {
            let report = entry.get("report").cloned().unwrap_or_default();
            let result = text(&report, "result").unwrap_or("ok");
            let note = text(&report, "note").filter(|n| !n.is_empty());
            for name in report
                .get("evidence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                evidence.push(json!({
                    "name": name,
                    "href": r#box.map(|b| artifact_href(b, name)),
                }));
            }
            note.map_or_else(
                || format!("Agent reported {result}{tries}"),
                |n| format!("Agent reported {result}{tries}: {n}"),
            )
        }
        "gate_refused" => {
            let missing: Vec<&str> = entry
                .get("missing")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            format!("Report refused{tries}: {}", missing.join("; "))
        }
        "reminded" => format!("Agent reminded to report{tries}"),
        "checked" => {
            let exit = entry.get("exit").and_then(Value::as_i64).unwrap_or(-1);
            format!("Check exited {exit}{tries}")
        }
        "check_lost" => format!("Check lost its stream; it runs again{tries}"),
        "answered" => {
            let answer = text(entry, "answer").unwrap_or("?");
            let note = text(entry, "note").filter(|n| !n.is_empty());
            note.map_or_else(
                || format!("Owner answered {answer}"),
                |n| format!("Owner answered {answer}: {n}"),
            )
        }
        "unanswered" => "No answer in time; the run ends declined".to_owned(),
        "exhausted" => "Out of tries; following the fail edge".to_owned(),
        "skipped" => "Skipped; the previous Box was still open".to_owned(),
        "refused" => format!("Refused: {}", text(entry, "detail").unwrap_or("no reason")),
        "ended" => {
            let outcome = entry.get("outcome").cloned().unwrap_or_default();
            format!(
                "Ended: {}",
                text(&outcome, "kind").unwrap_or("done").replace('_', " ")
            )
        }
        // A kind this build does not know reads as the line it was stored as.
        other => format!("{other}: {entry}"),
    };
    json!({"at": at, "kind": kind, "text": sentence, "evidence": evidence})
}

/// How a step stands in the run: what the last thing recorded for it says, unless the run is
/// at it now.
fn step_state(id: &str, run: &RunRowView, lines: &[&Value]) -> &'static str {
    let at_it = run.run.step.as_deref() == Some(id);
    if at_it && run.run.phase == "waiting" {
        return "waiting";
    }
    if at_it && run.run.phase == "running" {
        return "running";
    }
    let verdict = lines.iter().rev().find_map(|e| match text(e, "kind")? {
        "reported" => {
            let ok = e.pointer("/report/result").and_then(Value::as_str) != Some("fail");
            Some(if ok { "passed" } else { "failed" })
        }
        "checked" => Some(if e.get("exit").and_then(Value::as_i64) == Some(0) {
            "passed"
        } else {
            "failed"
        }),
        "answered" => Some(if text(e, "answer") == Some("yes") {
            "passed"
        } else {
            "failed"
        }),
        "exhausted" | "unanswered" => Some("failed"),
        _ => None,
    });
    match (verdict, lines.is_empty(), at_it) {
        (Some(v), _, _) => v,
        (None, true, _) => "pending",
        (None, false, true) => "failed",
        (None, false, false) => "ended",
    }
}

/// The run page's data: the run, its question when it waits, and each step of the frozen plan.
fn run_view(workflow: &str, row: &RunRowView) -> Value {
    let run = &row.run;
    let (started, ended) = span(run);
    let steps_doc: Vec<Value> = row
        .plan
        .as_ref()
        .and_then(|p| p.get("steps"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let r#box = run.box_name.as_deref();
    let entries: Vec<(i64, &Value)> = run.history.iter().map(|l| (l.at, &l.entry)).collect();
    let steps: Vec<Value> = steps_doc
        .iter()
        .map(|step| {
            let id = text(step, "id").unwrap_or_default();
            let mine: Vec<(i64, &Value)> = entries
                .iter()
                .filter(|(_, e)| text(e, "step") == Some(id))
                .copied()
                .collect();
            let raw: Vec<&Value> = mine.iter().map(|(_, e)| *e).collect();
            let history: Vec<Value> = mine.iter().map(|(at, e)| line_of(*at, e, r#box)).collect();
            let check = raw
                .iter()
                .rev()
                .find(|e| text(e, "kind") == Some("checked"))
                .map(|e| {
                    json!({
                        "exit": e.get("exit").and_then(Value::as_i64),
                        "try_n": e.get("try_n"),
                        "tail": clipped(text(e, "tail").unwrap_or_default()),
                    })
                });
            let expected: Vec<Value> = step
                .get("evidence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|name| json!({"name": name, "href": null}))
                .collect();
            let mut out = Map::new();
            for key in [
                "id",
                "title",
                "kind",
                "instructions",
                "done_when",
                "command",
                "question",
                "ok",
                "fail",
                "max_tries",
            ] {
                if let Some(v) = step.get(key) {
                    out.insert(key.to_owned(), v.clone());
                }
            }
            out.insert("state".to_owned(), json!(step_state(id, row, &raw)));
            out.insert("history".to_owned(), json!(history));
            out.insert("check".to_owned(), json!(check));
            out.insert("expects".to_owned(), json!(expected));
            Value::Object(out)
        })
        .collect();
    let in_plan = |id: &str| steps_doc.iter().any(|s| text(s, "id") == Some(id));
    let loose: Vec<Value> = entries
        .iter()
        .filter(|(_, e)| text(e, "step").is_none_or(|s| !in_plan(s)))
        .map(|(at, e)| line_of(*at, e, r#box))
        .collect();
    let question = (run.phase == "waiting")
        .then(|| {
            let at = run.step.as_deref()?;
            let step = steps_doc.iter().find(|s| text(s, "id") == Some(at))?;
            (text(step, "kind") == Some("approve"))
                .then(|| json!({"step": at, "text": text(step, "question").unwrap_or_default()}))
        })
        .flatten();
    json!({
        "workflow": workflow,
        "id": row.id,
        "repo": run.repo,
        "due_at": run.due_at,
        "started_at": started,
        "ended_at": ended,
        "duration_ms": started.zip(ended).map(|(s, e)| (e - s).max(0)),
        "phase": run.phase,
        "outcome": run.outcome,
        "status": status(&run.phase, &run.outcome),
        "label": label(&run.phase, &run.outcome, run.step.as_deref()),
        "step": run.step,
        "result": run.result,
        "detail": run.detail,
        "box": run.box_name,
        "question": question,
        "steps": steps,
        "lines": loose,
    })
}

/// Answers the approve step run `id` waits at, as the session's person, when the request
/// carries the CSRF token. The controller keeps the answer to the workflow's owner.
pub(super) fn answer(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    (name, id): (&str, i64),
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let parsed = body.and_then(|b| serde_json::from_slice::<AnswerBody>(b).ok());
    let answer = match parsed.as_ref().map(|b| b.answer.as_str()) {
        Some("yes") => Answer::Yes,
        Some("no") => Answer::No,
        _ => {
            let error = json!({"error": "answer yes or no"});
            return json(stream, "400 Bad Request", &error);
        }
    };
    let name = decode(name);
    let row = match one_run_row(gate, session, &name, id) {
        Ok(Some(row)) => row,
        Ok(None) => return json(stream, "404 Not Found", &json!({"error": "no such run"})),
        Err(Failure::Reply(reply)) => return answered(stream, &reply),
        Err(Failure::Other(why)) => return failed(stream, "run read", &why),
    };
    if row.run.phase != "waiting" {
        let error = json!({"error": "this run is not waiting for an answer"});
        return json(stream, "409 Conflict", &error);
    }
    let note = parsed
        .and_then(|b| b.note)
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    let request = ControllerRequest::WorkflowAnswer(WorkflowAnswerReq {
        identity: None,
        workflow: name.clone(),
        repo: row.run.repo.clone(),
        answer,
        note,
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) if reply.class == ExitClass::Ok => run(stream, gate, session, &name, id),
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "run answer", &why),
    }
}

/// Sends the bytes of the artifact the query names, as a download, read as the session's person.
pub(super) fn artifact(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    use std::io::Write as _;

    use crate::artifact::{self, Wanted};

    let pairs: std::collections::HashMap<String, String> =
        oauth2::url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let version = pairs.get("version").and_then(|v| v.parse().ok());
    let refuse = |stream: &mut TcpStream, refusal: &artifact::Refusal| {
        json(
            stream,
            refusal.status(),
            &json!({"error": refusal.message()}),
        );
    };
    let wanted = Wanted::parse(
        pairs.get("box").map_or("", String::as_str),
        pairs.get("name").map_or("", String::as_str),
        version,
    );
    let found = wanted.and_then(|w| artifact::find(&gate.controller, &session.person, &w));
    let found = match found {
        Ok(found) => found,
        Err(refusal) => return refuse(stream, &refusal),
    };
    if found.record.size > DOWNLOAD_MAX {
        let why =
            artifact::Refusal::BadRequest("that artifact is too large to download here".to_owned());
        return refuse(stream, &why);
    }
    let mut bytes = Vec::new();
    let read = artifact::read(&gate.controller, &session.person, &found, |data| {
        bytes.extend_from_slice(data);
        Ok(())
    });
    if let Err(refusal) = read {
        return refuse(stream, &refusal);
    }
    // The bytes are the agent's: a download, never a page, and sandboxed if a browser opens it.
    let file: String = found
        .record
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{file}\"\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(&bytes);
}
