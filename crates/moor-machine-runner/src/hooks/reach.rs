//! Running a hook or one of the runner's own scripts on a machine, over ssh or its agent link.

use moor_client::link::ops::{self, Hook, Op};

use super::{succeeded, Hooks};
use crate::caller::Reach;
use crate::exec::{self, ExecOutput};
use crate::link::{self, Answer};

impl Hooks {
    /// Runs hook `hook` for Box `id` with `env` in the Box's copy: over ssh the runner's command
    /// for it, over a link the machine's own file of that name. Its output when it exits 0, none
    /// when there is no such hook, else an error quoting the output's tail.
    pub(super) async fn hook(
        &self,
        id: i64,
        reach: &Reach,
        hook: Hook,
        env: &[(String, String)],
    ) -> Result<Option<ExecOutput>, String> {
        let (machine_id, machine) = match reach {
            Reach::Ssh { ssh } => {
                let command = match hook {
                    Hook::Prepare => self.prepare.as_ref().map(|c| {
                        format!(
                            "f=$(mktemp) || exit 1; ( {c}\n ) >\"$f\"; rc=$?; cat \"$f\"; \
                             rm -f \"$f\"; exit $rc"
                        )
                    }),
                    Hook::Suspend => self.suspend.clone(),
                    Hook::Release => self.release.clone(),
                };
                let Some(command) = command else {
                    return Ok(None);
                };
                let body = ops::in_box(id, &command);
                let out = exec::run(ssh, &exec::script(env, &body), self.timeout)
                    .await
                    .map_err(|e| format!("could not run ssh: {e}"))?;
                return succeeded(out).map(Some);
            }
            Reach::Linked {
                machine_id,
                machine,
            } => (*machine_id, machine),
        };
        let op = Op::Hook {
            box_id: id,
            hook,
            env: env.to_vec(),
            timeout_secs: link::secs(self.timeout),
        };
        match link::send(self.agents.as_ref(), machine, machine_id, &op).await? {
            Answer::Ran(ran) => succeeded(ran.into()).map(Some),
            Answer::Missing => Ok(None),
            Answer::Started(_) | Answer::Sending(..) => {
                Err(format!("machine {machine} answered no run"))
            }
        }
    }

    /// Runs the runner's own `script` for Box `id` on the machine, from the machine user's home
    /// over ssh and from the Box's copy over a link.
    pub(super) async fn helper(
        &self,
        id: i64,
        reach: &Reach,
        script: &str,
    ) -> Result<ExecOutput, String> {
        let (machine_id, machine) = match reach {
            Reach::Ssh { ssh } => {
                return exec::run(ssh, script, self.timeout)
                    .await
                    .map_err(|e| format!("could not run the remote shell: {e}"));
            }
            Reach::Linked {
                machine_id,
                machine,
            } => (*machine_id, machine),
        };
        let op = Op::Exec {
            box_id: id,
            env: Vec::new(),
            script: script.to_owned(),
            timeout_secs: link::secs(self.timeout),
        };
        match link::send(self.agents.as_ref(), machine, machine_id, &op).await? {
            Answer::Ran(ran) => Ok(ran.into()),
            _ => Err(format!("machine {machine} answered no run")),
        }
    }
}
