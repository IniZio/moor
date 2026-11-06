//! A run's own thread: owed when the run fires, posted, and opened; the Box is adopted into it
//! once the thread is open and the Box exists, in either order.

use moor_core::name::BoxName;
use moor_core::request::{Request, ShowReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::LastRunView;

use super::{Fx, GIVE_UP};
use crate::db::{Db, RunThread};
use crate::error_text::error_text;
use crate::escape::code;

/// What a run's change asks of its thread.
pub(super) enum RunStep {
    /// Owe the run a thread.
    Owe(RunThread),
    /// Record the id and branch of the Box of a thread already owed.
    Learn(String, i64, Option<String>),
}

/// What `run` asks of its thread: a run that is starting owes one, keyed by the name its Box
/// will have; a run with a Box owes one if none was, and tells an owed one the Box's id.
pub(super) fn run_thread(
    db: &Db,
    owner: &str,
    workflow: &str,
    (channel, origin_ts, run): (&str, &str, &LastRunView),
    fx: &mut Fx<'_>,
) -> Result<Option<RunStep>, String> {
    let known = |e: rusqlite::Error| format!("slack.db: {e}");
    let owed = |name: &str, found: Option<(i64, Option<String>)>| {
        RunStep::Owe(RunThread {
            box_name: name.to_owned(),
            owner: owner.to_owned(),
            workflow: workflow.to_owned(),
            repo: run.repo.clone(),
            channel: channel.to_owned(),
            origin_ts: origin_ts.to_owned(),
            box_id: found.as_ref().map(|f| f.0),
            branch: found.and_then(|f| f.1),
        })
    };
    if run.outcome == "starting" {
        let Some(name) = run.planned_box.as_deref() else {
            return Ok(None);
        };
        let new = db.run_thread_has_box(name).map_err(known)?.is_none();
        return Ok((new && db.box_thread(name).map_err(known)?.is_none()).then(|| owed(name, None)));
    }
    let Some(name) = run.box_name.as_deref() else {
        return Ok(None);
    };
    let Ok(box_name) = BoxName::parse_allow_bare(name) else {
        return Ok(None);
    };
    let state = db.run_thread_has_box(name).map_err(known)?;
    if state == Some(true) || db.box_thread(name).map_err(known)?.is_some() {
        return Ok(None);
    }
    let reply = (fx.call)(Request::Show(ShowReq {
        target: Some(box_name.into()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    }))
    .map_err(|e| format!("controller: {e}"))?;
    let boxed = reply.value.as_ref().and_then(|v| v.get("box"));
    let id = boxed
        .and_then(|b| b.get("id"))
        .and_then(serde_json::Value::as_i64);
    let Some(id) = id.filter(|_| reply.class == ExitClass::Ok) else {
        (fx.log)(&format!(
            "workflows: Box {name} cannot be adopted: {}",
            error_text(reply)
        ));
        return Ok(None);
    };
    let branch = boxed
        .and_then(|b| b.get("branch"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(Some(if state.is_some() {
        RunStep::Learn(name.to_owned(), id, branch)
    } else {
        owed(name, Some((id, branch)))
    }))
}

/// Posts the first message of each run thread `owner` is owed, then opens the thread with its
/// Box adopted. A post that fails stays owed and is given up on after [`GIVE_UP`] failures,
/// leaving any Box unadopted.
pub(super) fn open_threads(db: &Db, owner: &str, fx: &mut Fx<'_>) {
    let owed = match db.owed_run_threads(owner, GIVE_UP) {
        Ok(owed) => owed,
        Err(e) => return (fx.log)(&format!("slack.db: {e}; reading again")),
    };
    for (thread, attempts) in owed {
        let text = start_line(&thread);
        let ts = match (fx.post)(&thread.channel, None, &text) {
            Ok(ts) => ts,
            Err(why) => {
                let (name, wf) = (&thread.box_name, &thread.workflow);
                if attempts + 1 >= GIVE_UP {
                    (fx.log)(&format!("slack: run thread for Box {name} of workflow {wf} on {} dropped, any Box stays unadopted: {why}", thread.repo));
                } else {
                    (fx.log)(&format!("slack: run thread for Box {name} failed: {why}"));
                }
                if let Err(e) = db.run_thread_failed(name) {
                    (fx.log)(&format!("slack.db: {e}"));
                }
                continue;
            }
        };
        if let Err(e) = db.open_run_thread(&thread, &ts, (fx.clock)()) {
            (fx.log)(&format!("slack.db: {e}"));
        }
    }
}

/// The first message of a run's thread: it mentions the owner and links the thread that
/// added the workflow.
fn start_line(t: &RunThread) -> String {
    let link = format!(
        "https://slack.com/archives/{}/p{}",
        t.channel,
        t.origin_ts.replace('.', "")
    );
    format!(
        "<@{}> workflow {} fired for {} (set up in <{link}|this thread>). This thread follows that run: its Box once it starts, and why if the start fails.",
        t.owner,
        code(&t.workflow),
        code(&t.repo)
    )
}
