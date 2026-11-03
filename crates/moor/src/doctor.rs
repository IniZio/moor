//! `moor doctor`: one line per first-run check, each with the fix beside it.
//!
//! The checks only read. They reuse the client's dial, the config loader, the crabbox adapter
//! and the memory rules of the scheduler, so the doctor and a real start agree.

mod gate;
mod nodes;
mod provider;
mod render;
mod setup;
mod surface;
#[cfg(test)]
mod tests;
mod user;

use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::name::ForgeRepo;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::{Request, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::SystemProc;
use crate::adapters::rng::SystemRng;
use crate::cli::{GlobalOpts, Output};
use crate::client::{self, ClientError, Target};
use crate::config_load::load_user;
use crate::env::Env;
use crate::wire::tool_paths;

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
    /// Runs crabbox and herdr.
    pub proc: &'a dyn Proc,
    /// The crabbox adapter.
    pub driver: &'a dyn Driver,
    /// Status of a GET, or why it failed.
    pub get: &'a dyn Fn(&str) -> Result<u16, String>,
    /// The gate's public URL, when known.
    pub public_url: Option<&'a str>,
    /// The repo the checkout's origin names, which decides the secrets a start needs.
    pub repo: Option<ForgeRepo>,
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
    let mut out = vec![
        setup::config(i),
        setup::controller(i),
        provider::check(i, &read),
    ];
    out.extend(provider::controller(i));
    out.push(nodes::check(i, &read));
    out.extend(user::checks(i));
    out.push(surface::check(i));
    out.extend(gate::check(i));
    out
}

/// Runs every check against this host and renders the report; exit 1 when one failed.
#[must_use]
pub fn run(env: &Env, opts: &GlobalOpts, public_url: Option<&str>) -> Output {
    let (config, config_error) = match load_user(env) {
        Ok(config) => (config, None),
        Err(e) => (UserConfig::default(), Some(e.error.to_string())),
    };
    let target = client::configured_target(env);
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let clock = SystemClock::from_env(env).unwrap_or_else(|_| SystemClock::real());
    let driver = Crabbox::new(Arc::clone(&proc), Arc::new(clock), Arc::new(SystemRng));
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
        proc: proc.as_ref(),
        driver: &driver,
        get: &get,
        public_url,
        repo,
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
