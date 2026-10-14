//! The proxy environment the agent starts with: the nonce arrives as a staged file, never as text.

use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::Driver;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxPath};

use super::gh_shim::gh_shim;

/// The staged nonce file name inside the credential directory.
pub(super) const NONCE_FILE: &str = "proxy-nonce";
/// The shell variable that holds the nonce while the environment is built; never exported.
const NONCE_VAR: &str = "moor_nonce";
/// Where the gh shim goes, relative to the credential directory.
const SHIM_DIR: &str = "bin";

/// The fetch refspec of the `moor` remote; a push records `refs/remotes/moor/<branch>` through it.
const MOOR_FETCH: &str = "+refs/heads/*:refs/remotes/moor/*";

/// One piece of a shell word: literal text or the nonce.
enum Part<'a> {
    /// Text quoted as-is.
    Lit(&'a str),
    /// The nonce, read back from the shell variable.
    Nonce,
}

/// The shell word for `parts`; the nonce comes from the shell variable and never as text.
fn word(parts: &[Part<'_>]) -> String {
    if parts.is_empty() {
        return "''".to_owned();
    }
    parts
        .iter()
        .map(|p| match p {
            Part::Lit(s) => quote(s),
            Part::Nonce => format!("\"${NONCE_VAR}\""),
        })
        .collect()
}

/// Splits `value` around every occurrence of `nonce` into shell word parts.
fn split_on_nonce<'a>(value: &'a [u8], nonce: &[u8]) -> Result<Vec<Part<'a>>, String> {
    let mut parts = Vec::new();
    let mut rest = value;
    loop {
        let split = rest
            .windows(nonce.len())
            .position(|w| w == nonce)
            .and_then(|i| rest.split_at_checked(i))
            .and_then(|(lit, tail)| Some((lit, tail.get(nonce.len()..)?)));
        let lit = split.map_or(rest, |(lit, _)| lit);
        if !lit.is_empty() {
            let text = std::str::from_utf8(lit).map_err(|_| "env value is not UTF-8".to_owned())?;
            parts.push(Part::Lit(text));
        }
        let Some((_, tail)) = split else {
            return Ok(parts);
        };
        parts.push(Part::Nonce);
        rest = tail;
    }
}

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Stages the nonce at `<cred>/proxy-nonce` (mode 0600, raw nonce bytes, no newline) and the gh shim at
/// `<cred>/bin/gh` (bytes and mode from `gh_shim()`), both via `put_file`.
pub(super) fn stage(
    driver: &dyn Driver,
    handle: &BoxHandle,
    cred: &BoxPath,
    proxy: &ProxyAddr,
    host_cwd: &Path,
) -> Result<(), String> {
    let nonce_path = cred_file(cred, NONCE_FILE)?;
    driver
        .put_file(handle, &nonce_path, proxy.nonce.expose(), 0o600, host_cwd)
        .map_err(|e| format!("could not stage the proxy nonce: {e}"))?;
    let shim_path = cred_file(cred, &format!("{SHIM_DIR}/gh"))?;
    let (bytes, mode) = gh_shim();
    driver
        .put_file(handle, &shim_path, bytes, mode, host_cwd)
        .map_err(|e| format!("could not stage the gh shim: {e}"))
}

/// `rel` under the credential directory.
fn cred_file(cred: &BoxPath, rel: &str) -> Result<BoxPath, String> {
    BoxPath::under(cred.as_str(), rel).map_err(|e| e.to_string())
}

/// Shell lines run just before the agent starts. They must not contain the nonce.
pub(super) fn script_lines(
    cred: &BoxPath,
    proxy: &ProxyAddr,
    env: &[EnvVar],
    origin: Option<&str>,
) -> Result<Vec<String>, String> {
    let nonce = proxy.nonce.expose();
    if nonce.is_empty() {
        return Err("the proxy nonce is empty".to_owned());
    }
    let file = quote(cred_file(cred, NONCE_FILE)?.as_str());
    let mut lines = vec![
        format!("{NONCE_VAR}=$(cat {file}) && rm -f {file} && [ -n \"${NONCE_VAR}\" ] || exit 1"),
        "unset GH_TOKEN GITHUB_TOKEN".to_owned(),
    ];
    for var in env {
        if !is_identifier(&var.name) {
            return Err(format!("{:?} is not a shell variable name", var.name));
        }
        let parts = split_on_nonce(var.value.expose(), nonce)?;
        lines.push(format!("export {}={}", var.name, word(&parts)));
    }
    let base = format!("http://127.0.0.1:{}/t/", proxy.box_port);
    let key_base = format!("url.{base}");
    let key = [
        Part::Lit(&key_base),
        Part::Nonce,
        Part::Lit("/github/.insteadOf"),
    ];
    let api = [Part::Lit(&base), Part::Nonce, Part::Lit("/github-api")];
    let pushable = origin.and_then(push_url).zip(origin.and_then(github_repo));
    let count = if pushable.is_some() { 3 } else { 1 };
    lines.push(format!("export GIT_CONFIG_COUNT={count}"));
    lines.push(format!("export GIT_CONFIG_KEY_0={}", word(&key)));
    lines.push("export GIT_CONFIG_VALUE_0=https://github.com/".to_owned());
    lines.push(format!("export MOOR_GH_API={}", word(&api)));
    match pushable {
        Some((url, repo)) => {
            // The remote URL stays nonce-free; the insteadOf entry rewrites it when git uses it.
            lines.push("export GIT_CONFIG_KEY_1=remote.moor.url".to_owned());
            lines.push(format!("export GIT_CONFIG_VALUE_1={}", quote(&url)));
            lines.push("export GIT_CONFIG_KEY_2=remote.moor.fetch".to_owned());
            lines.push(format!("export GIT_CONFIG_VALUE_2={}", quote(MOOR_FETCH)));
            lines.push(format!("export MOOR_PUSH_URL={}", quote(&url)));
            let slug = format!("{}/{}", repo.owner, repo.name);
            lines.push(format!("export MOOR_GH_REPO={}", quote(&slug)));
        }
        None => lines.push("unset MOOR_PUSH_URL MOOR_GH_REPO".to_owned()),
    }
    for name in ["NO_PROXY", "no_proxy"] {
        lines.push(format!(
            "export {name}=\"127.0.0.1,localhost${{{name}:+,${name}}}\""
        ));
    }
    let bin = cred_file(cred, SHIM_DIR)?;
    lines.push(format!("export PATH={}:\"$PATH\"", quote(bin.as_str())));
    lines.push(format!("unset {NONCE_VAR}"));
    Ok(lines)
}

/// The GitHub repo an origin names: `https://github.com/o/n(.git)`, `ssh://git@github.com/o/n(.git)`,
/// `git@github.com:o/n(.git)`. None for any other host or shape.
pub(super) fn github_repo(origin: &str) -> Option<ForgeRepo> {
    let path = [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|p| origin.strip_prefix(p))?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (plain(owner) && plain(name)).then(|| ForgeRepo::new(owner, name))
}

/// `https://github.com/<owner>/<name>.git` for a GitHub origin, so the git `insteadOf` rewrite applies to pushes.
pub(super) fn push_url(origin: &str) -> Option<String> {
    let repo = github_repo(origin)?;
    Some(format!(
        "https://github.com/{}/{}.git",
        repo.owner, repo.name
    ))
}

#[cfg(test)]
mod tests;
