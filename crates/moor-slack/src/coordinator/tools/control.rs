//! The tools that act on a running Box and leave the thread's rows alone: `show_box` reads,
//! `interrupt_box`, `sleep_box`, `wake_box` and `restart_agent` change the Box through the
//! controller. Each of the four writes a journal row before it acts and works only for the
//! Box's requester; `show_box` is open to the thread.

use moor_core::request::{BoxReq, EndSessionReq, Request, ShowReq, StartReq, SteerReq};
use moor_core::response::ExitClass;
use moor_core::seam::forge::ForgeRepo;
use serde_json::Value;

use super::{changeable, error_text, not_running, thread_box, Controller, Reply, Scope};
use crate::db::SlackBox;

/// Most pane lines `show_box` hands the model.
const PANE_LINES: usize = 40;

/// What `end-session` says when the Box has no agent session to end.
const NO_LIVE_SESSION: &str = "no live Session";

/// What `start --resume` says when the Box holds no agent session to resume.
const NOTHING_TO_RESUME: &str = "no agent session to resume";

/// How the controller answered one request.
enum Said {
    /// It did what was asked.
    Done,
    /// It refused, with this message.
    Refused(String),
    /// No answer came, so the request may or may not have run.
    Lost(String),
}

/// Sends `request` and reads the answer.
fn ask(call: Controller<'_>, request: Request) -> Said {
    match call(request) {
        Ok(r) if r.class == ExitClass::Ok => Said::Done,
        Ok(r) => Said::Refused(error_text(r)),
        Err(e) => Said::Lost(format!("the controller did not answer: {e}")),
    }
}

/// The outcome and reply for a request that did not do what it asked, or none when it did.
/// `not` finishes the sentence "Not ...", as in `interrupted`.
fn failed(said: &Said, not: &str) -> Option<(String, Reply)> {
    match said {
        Said::Done => None,
        Said::Refused(why) => Some((
            format!("the controller refused: {why}"),
            Reply::refused(format!("Not {not}: {why}")),
        )),
        Said::Lost(why) => Some((
            why.clone(),
            Reply::refused(format!("Maybe not {not}: {why}")),
        )),
    }
}

/// The journal row, then `act` on the Box, then the row's outcome, for a Box the requester's
/// turn may change and that runs.
fn journaled(
    scope: &mut Scope<'_>,
    tool: &str,
    box_name: &str,
    verb: &str,
    act: impl FnOnce(Controller<'_>, &SlackBox) -> (String, Reply),
) -> rusqlite::Result<Reply> {
    let target = match changeable(scope, tool, box_name, "")? {
        Ok(target) => target,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(refusal) = not_running(&target.boxed, verb) {
        return Ok(refusal);
    }
    let db = scope.db;
    let seq = db.begin_action(target.turn.id, &target.action, (scope.clock)())?;
    let (outcome, reply) = act(&mut *scope.call, &target.boxed);
    db.end_action(target.turn.id, seq, &outcome, (scope.clock)())?;
    Ok(reply)
}

/// One request that either did what it asked or did not: `did` is the journal outcome and
/// `done` the reply when it did.
fn once(
    call: Controller<'_>,
    request: Request,
    (did, done, not): (String, String, &str),
) -> (String, Reply) {
    let said = ask(call, request);
    failed(&said, not).unwrap_or_else(|| (did, Reply::done(done)))
}

/// `interrupt_box`: the agent's interrupt key, through `steer`.
pub(super) fn interrupt(scope: &mut Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    journaled(scope, "interrupt_box", box_name, "interrupt", |call, b| {
        let name = b.box_name.as_str();
        let request = Request::Steer(SteerReq {
            target: b.box_name.clone().into(),
            text: None,
            menu: None,
            interrupt: true,
        });
        let words = (
            format!("interrupted Box `{name}`"),
            format!("Interrupted Box `{name}`."),
            "interrupted",
        );
        once(call, request, words)
    })
}

/// `sleep_box`.
pub(super) fn sleep(scope: &mut Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    journaled(scope, "sleep_box", box_name, "sleep", |call, b| {
        let name = b.box_name.as_str();
        let request = Request::Sleep(BoxReq {
            box_name: b.box_name.clone(),
        });
        let words = (
            format!("put Box `{name}` to sleep"),
            format!("Put Box `{name}` to sleep."),
            "put to sleep",
        );
        once(call, request, words)
    })
}

/// `wake_box`.
pub(super) fn wake(scope: &mut Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    journaled(scope, "wake_box", box_name, "wake", |call, b| {
        let name = b.box_name.as_str();
        let request = Request::Wake(BoxReq {
            box_name: b.box_name.clone(),
        });
        let words = (
            format!("woke Box `{name}`"),
            format!("Woke Box `{name}`."),
            "woken",
        );
        once(call, request, words)
    })
}

/// `restart_agent`: end the agent's session, then start it again with `resume`; when the Box
/// holds no session to resume, start its prompt over.
pub(super) fn restart(scope: &mut Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    journaled(scope, "restart_agent", box_name, "restart", |call, b| {
        let name = b.box_name.as_str();
        let end = Request::EndSession(EndSessionReq {
            box_name: name.to_owned(),
        });
        let ended = ask(&mut *call, end);
        let nothing_to_end =
            matches!(&ended, Said::Refused(why) if why.starts_with(NO_LIVE_SESSION));
        if !nothing_to_end {
            if let Some(stopped) = failed(&ended, "restarted") {
                return stopped;
            }
        }
        let repo = match ForgeRepo::parse(&b.repo) {
            Ok(repo) => repo,
            Err(e) => {
                let said = format!("the Box's repo is not usable: {e}");
                return (
                    said.clone(),
                    Reply::refused(format!("Not restarted: {said}")),
                );
            }
        };
        let resumed = ask(&mut *call, start_request(b, &repo, true));
        let from_prompt = matches!(&resumed, Said::Refused(why) if why == NOTHING_TO_RESUME);
        let said = if from_prompt {
            ask(call, start_request(b, &repo, false))
        } else {
            resumed
        };
        if let Some(stopped) = failed(&said, "restarted") {
            return stopped;
        }
        let how = if from_prompt {
            " from its prompt; it had no session to resume"
        } else {
            ", resuming its session"
        };
        let did = format!("restarted the agent of Box `{name}`{how}");
        let done = format!("Restarted the agent of Box `{name}`{how}.");
        (did, Reply::done(done))
    })
}

/// The `start` request that launches the agent of the Box `b` again: its session when
/// `resume`, else its prompt.
fn start_request(b: &SlackBox, repo: &ForgeRepo, resume: bool) -> Request {
    Request::Start(StartReq {
        name: b.box_name.clone(),
        prompt: (!resume).then(|| b.prompt.clone()),
        issue: None,
        base: None,
        model: None,
        node: b.node.clone(),
        no_agent: false,
        resume,
        repo: Some(repo.clone()),
        parent: b.parent,
        identity: None,
    })
}

/// `show_box`: the agent's state, what it waits on and the end of its pane. Anyone in the
/// thread may ask, so it reads only.
pub(super) fn show(scope: &mut Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    let boxed = match thread_box(scope, box_name)? {
        Ok(boxed) => boxed,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(refusal) = not_running(&boxed, "show") {
        return Ok(refusal);
    }
    let request = Request::Show(ShowReq {
        target: Some(boxed.box_name.clone().into()),
        events: None,
        agent: true,
        reports: false,
        artifacts: false,
        pane: true,
    });
    Ok(match (scope.call)(request) {
        Ok(r) if r.class == ExitClass::Ok => Reply::done(describe(
            boxed.box_name.as_str(),
            &r.value.unwrap_or(Value::Null),
        )),
        Ok(r) => Reply::refused(format!("The controller did not say: {}", error_text(r))),
        Err(e) => Reply::refused(format!("The controller is not answering: {e}")),
    })
}

/// What the `show` reply `view` says of Box `name`, with each line that comes from the Box in
/// a Box quote.
fn describe(name: &str, view: &Value) -> String {
    let text = |path: &[&str]| {
        let found = path.iter().try_fold(view, |v, key| v.get(key));
        found.and_then(Value::as_str)
    };
    let state = text(&["box", "agent", "state"]).unwrap_or("unknown");
    let mut out = format!(
        "Box `{name}`: lifecycle {}, agent {state}, branch {}.",
        text(&["box", "lifecycle"]).unwrap_or("unknown"),
        text(&["box", "branch"]).unwrap_or("unknown")
    );
    let first_run = text(&["box", "agent", "source"]) == Some("first_run");
    let question = text(&["box", "agent", "question"]).filter(|q| !q.is_empty());
    if first_run {
        out.push_str(
            "\nThe agent waits on a first-run screen. Typing does not skip it; it is a moor bug.",
        );
    }
    if let Some(question) = question {
        let what = if first_run {
            "The screen"
        } else {
            "Open question"
        };
        out.push('\n');
        out.push_str(what);
        out.push_str(", in a Box quote:");
        out.push_str(&quoted(question));
        let options = view.pointer("/box/agent/options").and_then(Value::as_array);
        for (i, option) in options.into_iter().flatten().enumerate() {
            let option = option.as_str().unwrap_or_default();
            out.push_str(&quoted(&format!("{}. {option}", i + 1)));
        }
    }
    if let Some(tail) = text(&["pane", "tail"])
        .map(str::trim_end)
        .filter(|t| !t.is_empty())
    {
        let lines: Vec<&str> = tail.lines().collect();
        let last = lines
            .get(lines.len().saturating_sub(PANE_LINES)..)
            .unwrap_or(&[]);
        out.push_str("\nThe end of its pane, in a Box quote:");
        out.push_str(&quoted(&last.join("\n")));
    }
    out
}

/// Each line of `text` on its own line starting with `> `.
fn quoted(text: &str) -> String {
    text.lines().fold(String::new(), |mut out, line| {
        out.push_str("\n  > ");
        out.push_str(line);
        out
    })
}
