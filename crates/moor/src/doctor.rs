//! `moor doctor`: one line per first-run check, each with the fix beside it.
//!
//! The checks only read. They reuse the client's dial, the config loader and the memory rules
//! of the scheduler, so the doctor and a real start agree.

mod gate;
mod host;
pub(crate) mod images;
mod nodes;
mod render;
mod setup;
#[cfg(test)]
mod tests;
mod user;

use moor_core::config::UserConfig;
use moor_core::name::ForgeRepo;
use moor_core::{Request, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::cli::{GlobalOpts, Output};
use crate::client::{self, ClientError, Target};
use crate::config_load::load_user;
use crate::env::Env;

pub(crate) use render::line;

/// Whether a check passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    /// Nothing to do.
    Ok,
    /// Works, but something is worth a look.
    Warn,
    /// A start will not work until this is fixed.
    Fail,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Check {
    /// What was checked.
    #[serde(rename = "check")]
    pub name: &'static str,
    /// The outcome.
    pub status: Status,
    /// What was found.
    pub detail: String,
    /// What to do about it; empty when `status` is `ok`.
    pub fix: String,
}

impl Check {
    /// A check that passed.
    pub(crate) fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self::with(name, Status::Ok, detail, "")
    }

    /// A check that passed with a caveat.
    pub(crate) fn warn(
        name: &'static str,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self::with(name, Status::Warn, detail, fix)
    }

    /// A check that failed.
    pub(crate) fn fail(
        name: &'static str,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self::with(name, Status::Fail, detail, fix)
    }

    /// A failed check from a [`Problem`].
    pub(crate) fn failed(name: &'static str, p: Problem) -> Self {
        Self::with(name, Status::Fail, p.detail, p.fix)
    }

    /// A check with every field given.
    fn with(
        name: &'static str,
        status: Status,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
            fix: fix.into(),
        }
    }
}

/// Why the controller could not answer, with the fix its client suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    /// What went wrong.
    pub detail: String,
    /// What to try.
    pub fix: String,
}

/// Sends one request to the controller.
pub(crate) type Ask<'a> = &'a dyn Fn(Request) -> Result<Response, ClientError>;

/// Everything the checks read, so a test can fake each part.
pub(crate) struct Inputs<'a> {
    /// The process environment.
    pub env: &'a Env,
    /// The user config, defaults when it did not load.
    pub config: &'a UserConfig,
    /// Why the user config did not load.
    pub config_error: Option<&'a str>,
    /// Where the controller is, when one is set.
    pub target: Option<&'a Target>,
    /// Asks the controller; only called when `target` is set.
    pub ask: Ask<'a>,
    /// Status of a GET, or why it failed.
    pub get: &'a dyn Fn(&str) -> Result<u16, String>,
    /// The gate's public URL, when known.
    pub public_url: Option<&'a str>,
    /// The repo the checkout's origin names, which decides the secrets a start needs.
    pub repo: Option<ForgeRepo>,
    /// Where an image is.
    pub present: &'a (dyn Fn(&str) -> images::Presence + Sync),
    /// The version the agent at a path reports.
    pub agent_version: &'a dyn Fn(&str) -> Option<String>,
    /// Whether an image that exists nowhere fails, as in the doctor, or only warns, as in `up`.
    pub images_fail: bool,
}

/// Sends `req` and reads its value as `T`.
pub(crate) fn answer<T: DeserializeOwned>(ask: Ask<'_>, req: Request) -> Result<T, Problem> {
    let cmd = req.cmd();
    let resp = match ask(req) {
        Ok(resp) => resp,
        Err(e) => {
            let failed = client::failure_response(cmd, &e);
            return Err(problem_of(&failed));
        }
    };
    if resp.error.is_some() {
        return Err(problem_of(&resp));
    }
    let value = resp.value.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| Problem {
        detail: format!("unreadable {cmd} answer: {e}"),
        fix: "run the same moor release on the CLI and the controller".to_owned(),
    })
}

/// The error of a failed `resp` as a problem.
fn problem_of(resp: &Response) -> Problem {
    let (detail, fix) = resp.error.as_ref().map_or_else(
        || (format!("{} failed", resp.cmd), String::new()),
        |e| {
            (
                e.message.clone(),
                e.hints.first().cloned().unwrap_or_default(),
            )
        },
    );
    Problem { detail, fix }
}

/// Every check, in the order of the setup guide.
pub(crate) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    let read = nodes::read(i);
    let mut out = vec![setup::config(i), setup::controller(i)];
    out.extend(host::checks(i));
    out.extend(images::checks(i));
    out.push(nodes::check(&read));
    out.extend(user::checks(i));
    out.extend(gate::check(i));
    out
}

/// Runs every check against this host and renders the report; exit 1 when one failed.
#[must_use]
pub fn run(env: &Env, opts: &GlobalOpts, public_url: Option<&str>) -> Output {
    run_with(env, opts, public_url, true)
}

/// [`run`], where an image that exists nowhere fails only when `images_fail`.
#[must_use]
pub(crate) fn run_with(
    env: &Env,
    opts: &GlobalOpts,
    public_url: Option<&str>,
    images_fail: bool,
) -> Output {
    let (config, config_error) = match load_user(env) {
        Ok(config) => (config, None),
        Err(e) => (UserConfig::default(), Some(e.error.to_string())),
    };
    let target = client::configured_target(env);
    let ask = |req: Request| {
        target.as_ref().map_or_else(
            || Err(ClientError::Usage("no controller is set".to_owned())),
            |t| client::call(env, t, &req, |_| {}, |_| {}),
        )
    };
    let repo = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::cli::caller_repo(&cwd, &crate::wire::host_checkout(env)).ok())
        .flatten();
    let get = http_status;
    let inputs = Inputs {
        env,
        config: &config,
        config_error: config_error.as_deref(),
        target: target.as_ref(),
        ask: &ask,
        get: &get,
        public_url,
        repo,
        present: &images::present,
        agent_version: &images::agent_version,
        images_fail,
    };
    render::output(&checks(&inputs), opts)
}

/// The status of a GET of `url`, without following a redirect.
fn http_status(url: &str) -> Result<u16, String> {
    let agent = crate::adapters::github::agent(std::time::Duration::from_secs(5));
    agent
        .get(url)
        .header("User-Agent", "moor-doctor")
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}
