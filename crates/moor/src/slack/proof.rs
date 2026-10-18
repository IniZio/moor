//! Proof files in Task threads: each `proof` event is uploaded, or explained, once.

use std::path::Path;

use moor_core::decide::proof::copy_relpath;
use moor_core::secret::Secret;
use moor_core::BoxId;
use serde_json::Value;

use super::api::Api;
use super::status::escape;
use crate::store::{SlackThread, Store};

/// One proof event waiting to be posted.
struct Queued {
    /// Key of the Box the proof belongs to; names its copy directory.
    id: BoxId,
    /// Task the proof belongs to.
    task: String,
    /// File name of the proof.
    name: String,
    /// Hash of the file content; with task and name it identifies the post.
    sha256: String,
    /// Why the controller kept no copy, when it refused.
    refused: Option<String>,
}

/// Proof events not yet posted, folded from the event feed.
#[derive(Default)]
pub(super) struct Proofs {
    /// Queued proofs in event order.
    queue: Vec<Queued>,
}

/// What posting one proof came to.
enum Outcome {
    /// Posted, or already posted; drop it.
    Done,
    /// Slack was unreachable; try again on the next poll.
    Retry,
}

impl Proofs {
    /// Queues a `proof` event; other kinds are ignored. A newer event for the same file
    /// replaces an older one that is still queued.
    pub(super) fn fold(&mut self, id: BoxId, task: &str, kind: &str, payload: &Value) {
        if kind != "proof" {
            return;
        }
        let text = |key: &str| payload.get(key).and_then(Value::as_str);
        let (Some(name), Some(sha256)) = (text("name"), text("sha256")) else {
            return;
        };
        self.queue.retain(|q| !(q.task == task && q.name == name));
        self.queue.push(Queued {
            id,
            task: task.to_owned(),
            name: name.to_owned(),
            sha256: sha256.to_owned(),
            refused: text("refused").map(str::to_owned),
        });
    }

    /// Posts every queued proof whose task has a thread and that the store has not recorded.
    /// Proofs for tasks without a thread stay queued, as do those Slack could not take.
    #[allow(
        clippy::too_many_arguments,
        reason = "the poll loop passes its handles individually"
    )]
    pub(super) fn post_due(
        &mut self,
        store: &Store,
        api: &Api,
        bot: &Secret,
        threads: &[SlackThread],
        state_dir: &Path,
        log: &dyn Fn(&str),
    ) {
        let cx = Cx {
            store,
            api,
            bot,
            state_dir,
            log,
        };
        self.queue.retain(|q| {
            let Some(thread) = threads.iter().find(|t| t.task.as_str() == q.task) else {
                return true;
            };
            matches!(post_one(&cx, thread, q), Outcome::Retry)
        });
    }
}

/// The handles one poll uses to post proofs.
struct Cx<'a> {
    /// Records what was posted.
    store: &'a Store,
    /// Slack client.
    api: &'a Api,
    /// Bot token for Slack calls.
    bot: &'a Secret,
    /// Directory holding the controller's proof copies.
    state_dir: &'a Path,
    /// Sink for diagnostics.
    log: &'a dyn Fn(&str),
}

/// Posts one proof into `thread` unless it was posted before.
fn post_one(cx: &Cx, thread: &SlackThread, q: &Queued) -> Outcome {
    let Cx {
        store,
        api,
        bot,
        state_dir,
        log,
    } = *cx;
    match store.slack_proof_posted(&q.task, &q.name, &q.sha256) {
        Ok(false) => {}
        Ok(true) => return Outcome::Done,
        Err(e) => {
            log(&format!("slack proof: {e}"));
            return Outcome::Retry;
        }
    }
    let name = escape(&q.name);
    let text = q.refused.as_ref().map_or_else(
        || {
            upload(api, bot, thread, state_dir, q)
                .err()
                .map(|why| format!("Proof {name} could not be uploaded: {}", escape(&why)))
        },
        |why| Some(format!("Proof {name} not uploaded: {}", escape(why))),
    );
    if let Some(text) = text {
        if let Err(e) = api.post_message(bot, &thread.channel, &thread.thread_ts, &text) {
            log(&format!("slack proof: {e}"));
            return Outcome::Retry;
        }
    }
    if let Err(e) = store.mark_slack_proof_posted(&q.task, &q.name, &q.sha256) {
        log(&format!("slack proof: {e}"));
    }
    Outcome::Done
}

/// Reads the controller's copy and uploads it to the thread; the error is a plain reason.
fn upload(
    api: &Api,
    bot: &Secret,
    thread: &SlackThread,
    state_dir: &Path,
    q: &Queued,
) -> Result<(), String> {
    let path = state_dir.join(copy_relpath(q.id, &q.name));
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the saved copy ({e})"))?;
    api.upload_file(bot, &thread.channel, &thread.thread_ts, &q.name, &bytes)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
