//! Requests over a linked machine's agent link: one stream each.

use moor_client::link::broker;
use moor_client::link::ops::{self, Op, Ran, Reply};
use moor_client::link::sweep::Swept;

use crate::caller::Reach;
use crate::Agents;
use moor_client::link::Channel;

/// What a request over the link came to.
pub(crate) enum Answer {
    /// The exec or hook ran.
    Ran(Ran),
    /// The relay started; its stdio follows on the stream.
    Started(Channel),
    /// The machine has no such hook or relay.
    Missing,
    /// The pulled file's bytes follow on the stream.
    Sending(Channel, u64),
    /// What a sweep removed.
    Swept(Swept),
}

/// Sends `op` to machine `machine_id`, named `machine` in errors, and reads its answer.
///
/// # Errors
/// The machine is offline, or its agent refused or answered something else.
pub(crate) async fn send(
    agents: Option<&Agents>,
    machine: &str,
    machine_id: i64,
    op: &Op,
) -> Result<Answer, String> {
    let offline = |why: String| format!("machine {machine} is offline: {why}");
    let agents = agents.ok_or_else(|| offline("this runner takes no agent links".to_owned()))?;
    let mut stream = agents.open(machine_id).await.map_err(offline)?;
    ops::write_line(&mut stream, op)
        .await
        .map_err(|e| offline(e.to_string()))?;
    match ops::read_line(&mut stream).await.map_err(offline)? {
        Reply::Ran(ran) => Ok(Answer::Ran(ran)),
        Reply::Started => Ok(Answer::Started(stream)),
        Reply::Missing => Ok(Answer::Missing),
        Reply::Sending { bytes } => Ok(Answer::Sending(stream, bytes)),
        Reply::Refused { reason } => Err(format!("machine {machine} refused: {reason}")),
        Reply::Swept(swept) => Ok(Answer::Swept(swept)),
        Reply::Listening | Reply::Status(_) => {
            Err(format!("machine {machine} answered something else"))
        }
    }
}

/// Sends `op`, an exec or a hook, to the machine `reach` names: over this runner's link, or
/// through the runner `reach` names as holding it when this one holds none.
///
/// # Errors
/// As [`send`], or the holder could not be reached.
pub(crate) async fn send_to(
    agents: Option<&Agents>,
    reach: &Reach,
    op: &Op,
) -> Result<Answer, String> {
    let here = agents.is_some_and(|a| a.online().contains(&reach.machine_id));
    let machine = &reach.machine;
    let Some(holder) = reach.holder.as_ref().filter(|_| !here) else {
        return send(agents, machine, reach.machine_id, op).await;
    };
    let offline = |why: String| format!("machine {machine} is offline: {why}");
    match broker::run(holder, reach.machine_id, op.clone())
        .await
        .map_err(offline)?
    {
        Reply::Ran(ran) => Ok(Answer::Ran(ran)),
        Reply::Missing => Ok(Answer::Missing),
        Reply::Refused { reason } => Err(format!("machine {machine} refused: {reason}")),
        _ => Err(format!("machine {machine} answered something else")),
    }
}

/// A timeout as the link carries it: whole seconds, 1 to [`ops::MAX_TIMEOUT_SECS`].
pub(crate) fn secs(timeout: std::time::Duration) -> u64 {
    timeout.as_secs().clamp(1, ops::MAX_TIMEOUT_SECS)
}
