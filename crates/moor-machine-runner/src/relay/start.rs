//! Starting a relay's server on the caller's machine, over ssh or its agent link.

use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use rmcp::ServiceExt;
use tokio::process::{Child, Command};
use tokio::time::Instant;

use moor_client::link::ops::Op;

use super::{expose, Forward, Relay, Relays, Session, Used};
use crate::caller::{Caller, Reach};
use crate::exec;
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
        let (service, child) = match &caller.reach {
            Reach::Ssh { ssh } => {
                let mut child = Self::over_ssh(caller, ssh, relay, &env)?;
                let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                    return Err("ssh has no stdio".to_owned());
                };
                let service = Forward(Arc::clone(&self.progress)).serve((stdout, stdin));
                (service.await, Some(child))
            }
            Reach::Linked { machine_id, .. } => {
                let op = Op::Relay {
                    box_id: caller.box_id.get(),
                    name: relay.name.clone(),
                    env,
                };
                let stream =
                    match link::send(self.agents.as_ref(), &caller.machine, *machine_id, &op)
                        .await?
                    {
                        Answer::Started(stream) => stream,
                        Answer::Missing => {
                            return Err(format!("machine {} has no such relay", caller.machine))
                        }
                        Answer::Ran(_) | Answer::Sending(..) => {
                            return Err(format!(
                                "machine {} ran the relay as a command",
                                caller.machine
                            ))
                        }
                    };
                let service = Forward(Arc::clone(&self.progress)).serve(tokio::io::split(stream));
                (service.await, None)
            }
        };
        let service = service.map_err(|e| format!("could not start: {e}"))?;
        let listed = service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| format!("could not list its tools: {e}"))?;
        let (tools, routes) = expose(relay, listed, taken)?;
        Ok(Session {
            service,
            child,
            tools,
            routes,
            used: Arc::new(Used {
                last: Mutex::new(Instant::now()),
                calls: AtomicUsize::new(0),
            }),
        })
    }
}

impl Relays {
    /// Starts `relay`'s command on the caller's machine over ssh, with `env`.
    fn over_ssh(
        caller: &Caller,
        ssh: &str,
        relay: &Relay,
        env: &[(String, String)],
    ) -> Result<Child, String> {
        let dir = caller.remote_dir();
        let prelude = hooks::state_prelude(caller.box_id.get());
        let body = format!(
            "{prelude}mkdir -p ~/{dir} && cd ~/{dir} && exec \"$SHELL\" -lc {}",
            exec::quote(&relay.command)
        );
        Command::new("ssh")
            .args(["-o", "BatchMode=yes", "--", ssh])
            .arg(exec::script(env, &body))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not run ssh: {e}"))
    }
}
