//! The proxy environment the agent starts with.

use moor_core::boxscript::quote;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose};
use moor_core::seam::agent_adapter::{EnvVar, ProxyAddr};
use moor_core::BoxPath;

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The Box environment the agent starts with, from the builder every exec uses. Without a `proxy` the
/// environment holds only the names to strip.
pub(super) fn exec_env(
    proxy: Option<&ProxyAddr>,
    strip: &[String],
    workdir: &BoxPath,
) -> Result<ExecEnv, String> {
    exec_env::build(
        &BoxCtx {
            env: proxy.map_or(&[], |p| &p.box_env),
            strip,
            proxy: proxy.map(ProxyCtx::of),
            workdir: Some(workdir),
        },
        Purpose::Agent,
    )
}

/// Shell lines run just before the agent starts: the builder's prelude, then the profile's `env`
/// exports.
pub(super) fn script_lines(proxy: &ProxyAddr, env: &[EnvVar]) -> Result<Vec<String>, String> {
    let mut lines = exec_env::build(
        &BoxCtx {
            env: &proxy.box_env,
            strip: &[],
            proxy: Some(ProxyCtx::of(proxy)),
            workdir: None,
        },
        Purpose::Agent,
    )?
    .prelude;
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
