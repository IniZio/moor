pub mod github_ops;
pub mod new;
pub mod old;
pub mod reports;

pub use reports::{ReportReq, ReportRow};
pub mod old_github;
mod old_proxy;
pub mod proxy;

pub use proxy::{ProxyCheck, ProxyEndpoint};
mod env;
mod types;

pub use env::{EnvItem, EnvOpts, EnvReport};
pub use types::*;

use std::path::{Path, PathBuf};

use crate::world::World;

pub trait Moor {
    fn name(&self) -> &'static str;
    fn caps(&self) -> &'static [&'static str] {
        &[]
    }
    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView>;
    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>>;
    fn close(&self, w: &World, cwd: &Path, name: &str, force: bool) -> Outcome<CloseView>;
    fn events(&self, w: &World, name: &str) -> Vec<Event>;
    fn env(&self, w: &World, cwd: &Path, opts: &EnvOpts) -> Outcome<EnvReport>;
    fn remove(&self, w: &World, cwd: &Path, name: &str, discard: bool) -> Outcome<RemoveView>;
    fn sessions(&self, w: &World, name: &str) -> Vec<Session>;
    fn age(&self, w: &World, name: &str, secs: i64);
    fn start_detached(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        opts: &StartOpts,
    ) -> std::process::Child;
    fn run(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &RunOpts,
    ) -> Outcome<RunResult>;
    fn cp(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        box_path: &str,
        host_path: &Path,
    ) -> Outcome<()>;
    fn gate_check(
        &self,
        w: &World,
        cwd: &Path,
        sha: &str,
        name: &str,
        command: Option<&str>,
    ) -> Outcome<GateVerdict>;
    fn runs(&self, w: &World, name: &str) -> Vec<RunRow>;
    fn seed_run(&self, w: &World, row: &RunRow);
    fn wait(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        timeout_secs: Option<u64>,
    ) -> Outcome<WaitView>;
    fn steer(&self, w: &World, cwd: &Path, name: &str, message: &str) -> Outcome<()>;
    fn open(&self, w: &World, cwd: &Path, name: &str) -> Outcome<OpenView>;
    fn shell(&self, w: &World, cwd: &Path, name: &str) -> Outcome<()>;
    fn pane(&self, w: &World, cwd: &Path, name: &str) -> Outcome<PaneView>;
    fn run_bg(&self, w: &World, cwd: &Path, name: &str, command: &str) -> std::process::Child;
    fn sleep_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView>;
    fn wake_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView>;
    fn report(&self, w: &World, cwd: &Path, name: &str, req: &ReportReq) -> Outcome<ReportRow>;
    fn reports(&self, w: &World, cwd: &Path, name: &str) -> Outcome<Vec<ReportRow>>;
    fn version(&self, w: &World) -> Outcome<String>;
    fn github_setup(
        &self,
        w: &World,
        cwd: &Path,
        opts: &github_ops::GithubSetupOpts,
    ) -> Outcome<github_ops::GithubStore>;
    fn github_login(&self, w: &World, cwd: &Path) -> Outcome<github_ops::GithubStore>;
    fn github_raw(&self, w: &World, cwd: &Path, args: &[&str]) -> Outcome<github_ops::GithubStore>;
    fn proxy_check(&self, w: &World, cwd: &Path, name: &str) -> Outcome<ProxyCheck>;
    fn proxy_endpoint(&self, w: &World, name: &str) -> Option<ProxyEndpoint>;
}

pub fn from_env() -> Result<Box<dyn Moor>, String> {
    let bin = std::env::var_os("MOOR_BIN").ok_or("MOOR_BIN is not set")?;
    let bin = PathBuf::from(bin);
    if !bin.exists() {
        return Err(format!("MOOR_BIN does not exist: {}", bin.display()));
    }
    match std::env::var("MOOR_CLI").as_deref() {
        Err(_) | Ok("old") => Ok(Box::new(old::OldCli { bin })),
        Ok("new") => Ok(Box::new(new::NewCli { bin })),
        Ok(other) => Err(format!("unknown MOOR_CLI: {other}")),
    }
}

pub fn exit_class(code: i32) -> ExitClass {
    match code {
        0 => ExitClass::Ok,
        2 => ExitClass::Usage,
        _ => ExitClass::Failed,
    }
}

#[cfg(test)]
mod tests;
