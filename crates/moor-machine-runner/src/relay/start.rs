//! Starting a relay's server on the caller's machine, over ssh or its agent link.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use rmcp::ServiceExt;
use tokio::time::Instant;

use moor_client::link::ops::Op;

use super::{expose, Forward, Relay, Relays, Session, Used};
use crate::caller::Caller;
use crate::hooks::{self, Facts};
use crate::link::{self, Answer};

impl Relays {
    /// Starts the relay on the caller's machine and lists its tools.
    pub(super) async fn start(
        &self,
        caller: &Caller,
        relay: &Relay,
        facts: &Facts,
        taken: &[String],
    ) -> Result<Session, String> {
        let env = hooks::box_env(caller.box_id.get(), caller.slot, facts);
        let machine_id = caller.reach.machine_id;
        let op = Op::Relay {
            box_id: caller.box_id.get(),
            name: relay.name.clone(),
            env,
        };
        let stream = match link::send(self.agents.as_ref(), &caller.machine, machine_id, &op)
            .await?
        {
            Answer::Started(stream) => stream,
            Answer::Missing => return Err(format!("machine {} has no such relay", caller.machine)),
            Answer::Ran(_) | Answer::Sending(..) => {
                return Err(format!(
                    "machine {} ran the relay as a command",
                    caller.machine
                ))
            }
        };
        let service = Forward(Arc::clone(&self.progress))
            .serve(tokio::io::split(stream))
            .await;
        let service = service.map_err(|e| format!("could not start: {e}"))?;
        let listed = service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| format!("could not list its tools: {e}"))?;
        let (tools, routes) = expose(relay, listed, taken)?;
        Ok(Session {
            service,
            tools,
            routes,
            used: Arc::new(Used {
                last: Mutex::new(Instant::now()),
                calls: AtomicUsize::new(0),
            }),
        })
    }
}
