//! One run of a workflow: the frozen plan joined to the run's history, and the answer to the
//! approve step it waits at.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowAnswerReq, WorkflowRunListReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::{RunRowView, WorkflowRunListValue};
use moor_core::workflow::{Answer, PullTarget};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{decode, not_found, span, status, value_of, verdict, Failure, TAIL_MAX};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// The body of an answer.
#[derive(Deserialize)]
struct AnswerBody {
    /// `yes` or `no`.
    answer: String,
    /// What the owner adds for the agent.
    #[serde(default)]
    note: Option<String>,
}

/// Answers one run: the frozen plan as steps, each with its history, check output and evidence.
pub(crate) fn run(stream: &mut TcpStream, gate: &Gate, session: &Session, name: &str, id: i64) {
    let name = decode(name);
    match one_run(gate, session, &name, id) {
        Ok(Some(view)) => json(stream, "200 OK", &view),
        Ok(None) => json(stream, "404 Not Found", &json!({"error": "no such run"})),
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Missing) => not_found(stream),
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
    let listed = Failure::own(value_of::<WorkflowRunListValue>(
        gate,
        session,
        ControllerRequest::WorkflowRunList(WorkflowRunListReq {
            identity: None,
            workflow: name.to_owned(),
            run: Some(id),
            limit: Some(1),
        }),
    ))?;
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
    tail.get(from..).unwrap_or_default()
}

/// The href that downloads artifact `name` of Box `r#box`.
fn artifact_href(r#box: &str, name: &str) -> String {
    let query = oauth2::url::form_urlencoded::Serializer::new(String::new())
        .append_pair("box", r#box)
        .append_pair("name", name)
        .finish();
    format!("/account/api/artifact?{query}")
}

/// The strings in `value`'s list `key`.
fn strings<'a>(value: &'a Value, key: &str) -> Vec<&'a str> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The evidence a report names, each with the link that downloads it from Box `r#box`.
fn evidence_of(report: &Value, r#box: Option<&str>) -> Vec<Value> {
    strings(report, "evidence")
        .into_iter()
        .map(|name| json!({"name": name, "href": r#box.map(|b| artifact_href(b, name))}))
        .collect()
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
            evidence = evidence_of(&report, r#box);
            note.map_or_else(
                || format!("Agent reported {result}{tries}"),
                |n| format!("Agent reported {result}{tries}: {n}"),
            )
        }
        "gate_refused" => {
            let missing = strings(entry, "missing");
            format!("Report refused{tries}: {}", missing.join("; "))
        }
        "reminded" => format!("Agent reminded to report{tries}"),
        "checked" => gate_line::checked(entry, &tries),
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
        "skipped_fork" => format!(
            "Skipped; the fork's author {} cannot write to the repo",
            text(entry, "author").unwrap_or("?")
        ),
        "refused" => format!("Refused: {}", text(entry, "detail").unwrap_or("no reason")),
        "close_unconfirmed" => format!(
            "Close not confirmed, the run stays open: {}",
            text(entry, "detail").unwrap_or("no reason")
        ),
        "ended" => format!("Ended: {}", verdict::ended_word(entry)),
        other => gate_line::sentence(other, entry, &tries),
    };
    let line = json!({"at": at, "kind": kind, "text": sentence, "evidence": evidence});
    gate_line::with_cause(line, entry)
}

/// How a step stands in the run: what the last thing recorded for it says, unless the run is
/// at it now.
fn step_state(id: &str, run: &RunRowView, lines: &[&Value]) -> &'static str {
    if verdict::ended_early(&run.run, lines) {
        return if lines.is_empty() { "pending" } else { "ended" };
    }
    if lines.iter().any(|e| text(e, "kind") == Some("gate_passed")) {
        return "passed";
    }
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
        "gate_passed" => Some("passed"),
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

/// One step of the frozen plan with what the history says about it.
fn step_view(
    step: &Value,
    row: &RunRowView,
    (entries, r#box): (&[(i64, &Value)], Option<&str>),
) -> Value {
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
        "checks",
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
        .map(|step| step_view(step, row, (&entries, r#box)))
        .collect();
    let in_plan = |id: &str| steps_doc.iter().any(|s| text(s, "id") == Some(id));
    let loose: Vec<Value> = entries
        .iter()
        .filter(|(_, e)| text(e, "step").is_none_or(|s| !in_plan(s)))
        .filter(|(_, e)| !verdict::told_by_end(e, &entries))
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
        "due_at": run.due_at,
        "started_at": started,
        "ended_at": ended,
        "duration_ms": started.zip(ended).map(|(s, e)| (e - s).max(0)),
        "phase": run.phase,
        "outcome": run.outcome,
        "status": status(&run.phase, &run.outcome),
        "label": verdict::label_of(run),
        "verdict": verdict::named(run),
        "closing": verdict::closing(run),
        "pull": run.pull,
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
pub(crate) fn answer(
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
        Err(Failure::Missing) => return not_found(stream),
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
        pull: row
            .run
            .pull
            .as_deref()
            .and_then(|p| PullTarget::parse(p).ok()),
        answer,
        note,
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) if reply.class == ExitClass::Ok => run(stream, gate, session, &name, id),
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "run answer", &why),
    }
}

mod gate_line;
#[cfg(test)]
mod tests;
