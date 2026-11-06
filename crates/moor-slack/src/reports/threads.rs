//! A run's own thread: owed when its Box first appears, posted and then opened with the Box
//! adopted into it.

use moor_core::name::BoxName;
use moor_core::request::{Request, ShowReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::LastRunView;

use super::{Fx, GIVE_UP};
use crate::db::{Db, RunThread};
use crate::error_text::error_text;
use crate::escape::code;

/// The run thread `run`'s Box is owed, when no thread has the Box and none was owed before.
pub(super) fn run_thread(
    db: &Db,
    owner: &str,
    workflow: &str,
    (channel, origin_ts, run): (&str, &str, &LastRunView),
    fx: &mut Fx<'_>,
) -> Result<Option<RunThread>, String> {
    let Some(name) = run.box_name.as_deref() else {
        return Ok(None);
    };
    let Ok(box_name) = BoxName::parse_allow_bare(name) else {
        return Ok(None);
    };
    let known = |e: rusqlite::Error| format!("slack.db: {e}");
    if db.box_thread(name).map_err(known)?.is_some() || db.run_thread_known(name).map_err(known)? {
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
        .and_then(serde_json::Value::as_str);
    Ok(Some(RunThread {
        box_name: name.to_owned(),
        owner: owner.to_owned(),
        workflow: workflow.to_owned(),
        repo: run.repo.clone(),
        channel: channel.to_owned(),
        origin_ts: origin_ts.to_owned(),
        box_id: id,
        branch: branch.map(str::to_owned),
    }))
}

/// Posts the first message of each run thread `owner` is owed, then opens the thread with its
/// Box adopted. A post that fails stays owed and is given up on after [`GIVE_UP`] failures,
/// leaving the Box unadopted.
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
                    (fx.log)(&format!("slack: run thread for Box {name} of workflow {wf} on {} dropped, the Box stays unadopted: {why}", thread.repo));
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
        "<@{}> workflow {} started on {} (set up in <{link}|this thread>). Its Box {} works here.",
        t.owner,
        code(&t.workflow),
        code(&t.repo),
        code(&t.box_name)
    )
}
