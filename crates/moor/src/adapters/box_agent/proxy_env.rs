//! The proxy environment the agent starts with: the nonce arrives as a staged file, never as text.

use moor_core::boxscript::quote;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, MacTask, ProxyCtx, Purpose};
use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::BoxStep;
use moor_core::BoxPath;

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The steps that stage the nonce and the public CA certificate under the credential directory, each
/// with the text a failure of that step is reported under.
pub(super) fn stage_steps(
    cred: &BoxPath,
    proxy: &ProxyAddr,
) -> Result<[(BoxStep, &'static str); 2], String> {
    exec_env::stage_steps(cred, &proxy.nonce, &proxy.ca_pem)
}

/// The Box environment the agent starts with, from the builder every exec uses. Without a `proxy` the
/// environment holds only the names to strip.
pub(super) fn exec_env(
    cred: &BoxPath,
    proxy: Option<&ProxyAddr>,
    strip: &[String],
    origin: Option<&str>,
    workdir: &BoxPath,
    mac: Option<MacTask<'_>>,
) -> Result<ExecEnv, String> {
    exec_env::build(
        &BoxCtx {
            env: proxy.map_or(&[], |p| &p.box_env),
            strip,
            proxy: proxy.map(|p| ProxyCtx {
                box_port: p.box_port,
                cred,
            }),
            origin,
            workdir: Some(workdir),
            mac,
        },
        Purpose::Agent,
    )
}

/// The Mac Task lines for a launch with no Proxy: the prelude that restores the recorded
/// `DEVELOPER_DIR`, then the `export` lines for the Mac Task variables.
pub(super) fn task_lines(mac: MacTask<'_>) -> Result<Vec<String>, String> {
    let built = exec_env::build(
        &BoxCtx {
            env: &[],
            strip: &[],
            proxy: None,
            origin: None,
            workdir: None,
            mac: Some(mac),
        },
        Purpose::Agent,
    )?;
    let mut lines = built.prelude;
    for (name, value) in &built.vars {
        lines.push(format!("export {name}={}", quote(value)));
    }
    Ok(lines)
}

/// Shell lines run just before the agent starts: the builder's prelude, the Mac task variables when there are any, then the profile's `env`
/// exports. They must not contain the nonce.
pub(super) fn script_lines(
    cred: &BoxPath,
    proxy: &ProxyAddr,
    env: &[EnvVar],
    origin: Option<&str>,
    mac: Option<MacTask<'_>>,
) -> Result<Vec<String>, String> {
    if proxy.nonce.expose().is_empty() {
        return Err("the proxy nonce is empty".to_owned());
    }
    let built = exec_env::build(
        &BoxCtx {
            env: &proxy.box_env,
            strip: &[],
            proxy: Some(ProxyCtx {
                box_port: proxy.box_port,
                cred,
            }),
            origin,
            workdir: None,
            mac,
        },
        Purpose::Agent,
    )?;
    let mut lines = built.prelude;
    if mac.is_some() {
        for (name, value) in &built.vars {
            lines.push(format!("export {name}={}", quote(value)));
        }
    }
    for var in env {
        if !is_identifier(&var.name) {
            return Err(format!("{:?} is not a shell variable name", var.name));
        }
        let value = std::str::from_utf8(var.value.expose())
            .map_err(|_| "env value is not UTF-8".to_owned())?;
        lines.push(format!("export {}={}", var.name, quote(value)));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests;
