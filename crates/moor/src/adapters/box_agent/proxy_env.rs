//! The proxy environment the agent starts with: the nonce arrives as a staged file, never as text.

use moor_core::boxscript::quote;
use moor_core::seam::agent_profile::{EnvVar, ProxyAddr};
use moor_core::seam::driver::BoxStep;
use moor_core::seam::forge::ForgeRepo;
use moor_core::BoxPath;

/// The staged nonce file name inside the credential directory.
pub(super) const NONCE_FILE: &str = "proxy-nonce";
/// The shell variable that holds the nonce while the environment is built; never exported.
const NONCE_VAR: &str = "moor_nonce";
/// The staged public CA certificate file name inside the credential directory.
pub(super) const CA_FILE: &str = "proxy-ca.pem";
/// The CA bundle built in the credential directory when the system bundle cannot be edited.
const BUNDLE_FILE: &str = "ca-bundle.pem";
/// System CA bundles, first existing one wins.
const SYSTEM_BUNDLES: &str =
    "/etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem";

/// The fetch refspec of the `moor` remote; a push records `refs/remotes/moor/<branch>` through it.
const MOOR_FETCH: &str = "+refs/heads/*:refs/remotes/moor/*";

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The steps that stage the nonce at `<cred>/proxy-nonce` (mode 0600, raw nonce bytes, no newline) and the
/// public CA certificate at `<cred>/proxy-ca.pem` (mode 0644), each with the text a failure of that step is reported under.
pub(super) fn stage_steps(
    cred: &BoxPath,
    proxy: &ProxyAddr,
) -> Result<[(BoxStep, &'static str); 2], String> {
    let nonce = BoxStep::Put {
        path: cred_file(cred, NONCE_FILE)?,
        bytes: proxy.nonce.expose().to_vec(),
        mode: 0o600,
    };
    let ca = BoxStep::Put {
        path: cred_file(cred, CA_FILE)?,
        bytes: proxy.ca_pem.as_bytes().to_vec(),
        mode: 0o644,
    };
    Ok([
        (nonce, "could not stage the proxy nonce"),
        (ca, "could not stage the proxy CA"),
    ])
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
    if proxy.nonce.expose().is_empty() {
        return Err("the proxy nonce is empty".to_owned());
    }
    let file = quote(cred_file(cred, NONCE_FILE)?.as_str());
    let ca = quote(cred_file(cred, CA_FILE)?.as_str());
    let built = quote(cred_file(cred, BUNDLE_FILE)?.as_str());
    let mut lines = vec![
        format!("{NONCE_VAR}=$(cat {file}) && rm -f {file} && [ -n \"${NONCE_VAR}\" ] || exit 1"),
        "unset GH_TOKEN GITHUB_TOKEN".to_owned(),
        "moor_sys=; for moor_f in ".to_owned() + SYSTEM_BUNDLES + "; do [ -f \"$moor_f\" ] && { moor_sys=$moor_f; break; }; done",
        format!("moor_mark=$(sed -n 2p {ca})"),
        format!(
            "if [ -n \"$moor_sys\" ] && sudo -n true 2>/dev/null && {{ grep -qF \"$moor_mark\" \"$moor_sys\" || sudo -n tee -a \"$moor_sys\" <{ca} >/dev/null; }}; then moor_bundle=$moor_sys; else moor_bundle={built}; {{ if [ -n \"$moor_sys\" ]; then cat \"$moor_sys\"; fi; cat {ca}; }} >\"$moor_bundle\" && chmod 644 \"$moor_bundle\"; fi"
        ),
    ];
    for name in [
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "REQUESTS_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        lines.push(format!("export {name}=\"$moor_bundle\""));
    }
    lines.push(format!("export NODE_EXTRA_CA_CERTS={ca}"));
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        lines.push(format!(
            "export {name}=\"http://moor:${{{NONCE_VAR}}}@127.0.0.1:{}\"",
            proxy.box_port
        ));
    }
    lines.extend(yarn_lines(proxy.box_port));
    for name in ["NO_PROXY", "no_proxy"] {
        lines.push(format!(
            "export {name}=\"127.0.0.1,localhost${{{name}:+,${name}}}\""
        ));
    }
    lines.push("export NODE_USE_ENV_PROXY=1".to_owned());
    for var in env {
        if !is_identifier(&var.name) {
            return Err(format!("{:?} is not a shell variable name", var.name));
        }
        let value = std::str::from_utf8(var.value.expose())
            .map_err(|_| "env value is not UTF-8".to_owned())?;
        lines.push(format!("export {}={}", var.name, quote(value)));
    }
    lines.extend(git_remote_lines(origin));
    lines.push(format!(
        "unset {NONCE_VAR} moor_bundle moor_sys moor_mark moor_f"
    ));
    Ok(lines)
}

/// Shell lines that point Yarn and npm at the proxy, the CA bundle and a cache the sandbox can write.
fn yarn_lines(box_port: u16) -> Vec<String> {
    let mut lines: Vec<String> = ["YARN_HTTPS_PROXY", "YARN_HTTP_PROXY"]
        .iter()
        .map(|name| format!("export {name}=\"http://moor:${{{NONCE_VAR}}}@127.0.0.1:{box_port}\""))
        .collect();
    lines.extend(
        [
            "export YARN_CA_FILE_PATH=\"$moor_bundle\"",
            "export YARN_CACHE_FOLDER=\"$HOME/.cache/yarn\"",
            "export YARN_GLOBAL_FOLDER=\"$HOME/.cache/yarn/berry\"",
            "export npm_config_cache=\"$HOME/.cache/npm\"",
        ]
        .map(str::to_owned),
    );
    lines
}

/// Shell lines that point the `moor` git remote at the push URL for `origin`, or clear the git config env.
fn git_remote_lines(origin: Option<&str>) -> Vec<String> {
    origin.and_then(push_url).map_or_else(
        || vec!["unset GIT_CONFIG_COUNT".to_owned()],
        |url| {
            vec![
                "export GIT_CONFIG_COUNT=2".to_owned(),
                "export GIT_CONFIG_KEY_0=remote.moor.url".to_owned(),
                format!("export GIT_CONFIG_VALUE_0={}", quote(&url)),
                "export GIT_CONFIG_KEY_1=remote.moor.fetch".to_owned(),
                format!("export GIT_CONFIG_VALUE_1={}", quote(MOOR_FETCH)),
            ]
        },
    )
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

/// `https://github.com/<owner>/<name>.git` for a GitHub origin, for the `moor` remote.
pub(super) fn push_url(origin: &str) -> Option<String> {
    let repo = github_repo(origin)?;
    Some(format!(
        "https://github.com/{}/{}.git",
        repo.owner, repo.name
    ))
}

#[cfg(test)]
mod tests;
