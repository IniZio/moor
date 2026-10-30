//! Running a hook or one of the runner's own scripts on a machine, over its agent link.

use moor_client::link::ops::{Hook, Op};

use super::{succeeded, Hooks};
use crate::caller::Reach;
use crate::exec::ExecOutput;
use crate::link::{self, Answer};

impl Hooks {
    /// Runs hook `hook` for Box `id` with `env` in the Box's copy: the machine's own file of that
    /// name. Its output when it exits 0, none
    /// when there is no such hook, else an error quoting the output's tail.
    pub(super) async fn hook(
        &self,
        id: i64,
        reach: &Reach,
        hook: Hook,
        env: &[(String, String)],
    ) -> Result<Option<ExecOutput>, String> {
        let Reach {
            machine_id,
            machine,
        } = reach;
        let op = Op::Hook {
            box_id: id,
            hook,
            env: env.to_vec(),
            timeout_secs: link::secs(self.timeout),
        };
        match link::send(self.agents.as_ref(), machine, *machine_id, &op).await? {
            Answer::Ran(ran) => succeeded(ran.into()).map(Some),
            Answer::Missing => Ok(None),
            Answer::Started(_) | Answer::Sending(..) => {
                Err(format!("machine {machine} answered no run"))
            }
        }
    }

    /// Runs the runner's own `script` for Box `id` on the machine, from the Box's copy.
    pub(super) async fn helper(
        &self,
        id: i64,
        reach: &Reach,
        script: &str,
    ) -> Result<ExecOutput, String> {
        let Reach {
            machine_id,
            machine,
        } = reach;
        let op = Op::Exec {
            box_id: id,
            env: Vec::new(),
            script: script.to_owned(),
            timeout_secs: link::secs(self.timeout),
        };
        match link::send(self.agents.as_ref(), machine, *machine_id, &op).await? {
            Answer::Ran(ran) => Ok(ran.into()),
            _ => Err(format!("machine {machine} answered no run")),
        }
    }
}
