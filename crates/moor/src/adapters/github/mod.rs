//! GitHub forge adapter: routes and repo naming come from the route table, and
//! the repo token comes from a host command run through the `Proc` seam.

mod routes;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use moor_core::proxy_gate::RouteRow;
use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo};
use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};
use moor_core::secret::Secret;

/// How long the token command may run.
const TOKEN_DEADLINE: Deadline = Deadline(Duration::from_secs(10));
/// Config key named in every error.
const KEY: &str = "github.you_token_cmd";

/// Exit codes `sh` uses when `exec` cannot find or run the program.
const EXEC_FAILED: [i32; 2] = [126, 127];

/// GitHub as a [`Forge`].
pub(crate) struct GitHub {
    /// Runs the token command.
    proc: Arc<dyn Proc>,
    /// Program and arguments that print the token.
    token_cmd: Vec<String>,
    /// Environment pairs for the token command.
    env: Vec<(String, String)>,
}

impl GitHub {
    /// `env` holds the already-resolved name/value pairs passed to the token command.
    pub(crate) fn new(
        proc: Arc<dyn Proc>,
        token_cmd: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Self {
        Self {
            proc,
            token_cmd,
            env,
        }
    }
}

impl Forge for GitHub {
    fn routes(&self) -> Vec<RouteRow> {
        routes::routes()
    }

    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        routes::parse_origin(origin)
    }

    /// Runs the token command and returns its trimmed stdout.
    ///
    /// The `repo` argument is not used for scoping, because a gh token is account-wide.
    /// Errors name `github.you_token_cmd` and never carry the token, stdout or stderr.
    fn repo_token(&self, _repo: &ForgeRepo) -> Result<Secret, ForgeError> {
        let mut spec = ProcSpec::new(Tool::Sh, TOKEN_DEADLINE);
        for (name, value) in &self.env {
            spec = spec.env(name.as_str(), value);
        }
        let spec = spec
            .args(["-c", "exec \"$@\"", "moor-github-token"])
            .args(&self.token_cmd);
        let out = self.proc.run_secret(&spec).map_err(|e| match e {
            ProcError::NonZero { code, .. } if EXEC_FAILED.contains(&code) => {
                ForgeError::Failed(format!("{KEY} could not be run (exit {code})"))
            }
            ProcError::NonZero { code, .. } => {
                ForgeError::Auth(format!("{KEY} exited {code}; check it is logged in"))
            }
            ProcError::Spawn(e) => {
                ForgeError::Failed(format!("{KEY} could not start: {}", e.kind()))
            }
            ProcError::Timeout { after, .. } => {
                ForgeError::Failed(format!("{KEY} timed out after {}s", after.as_secs()))
            }
            ProcError::Signalled(_) | ProcError::Interrupted => {
                ForgeError::Failed(format!("{KEY} was killed by a signal"))
            }
        })?;
        let token = out.trim_ascii();
        if token.is_empty() {
            return Err(ForgeError::Auth(format!("{KEY} printed no token")));
        }
        Ok(Secret::new(token.to_vec()))
    }
}
