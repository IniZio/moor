//! The one Box environment every exec uses: the same variables for every purpose, only the
//! deadline and pty differ.

use std::time::Duration;

use crate::boxscript::quote;
use crate::name::BoxPath;
use crate::name::DirectHost;
use crate::seam::driver::BoxStep;
use crate::seam::forge::ForgeRepo;
use crate::seam::proc::Deadline;

/// The staged public CA certificate file name inside the credential directory.
pub const CA_FILE: &str = "proxy-ca.pem";
/// The CA bundle built in the credential directory when the system bundle cannot be edited.
const BUNDLE_FILE: &str = "ca-bundle.pem";
/// System CA bundles, first existing one wins.
const SYSTEM_BUNDLES: &str =
    "/etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem";
/// Where an image can record the `PATH` its Dockerfile `ENV` sets; non-interactive ssh execs do
/// not inherit that `ENV`.
const IMAGE_ENV_FILE: &str = "/etc/environment";
/// The fetch refspec of the `moor` remote; a push records `refs/remotes/moor/<branch>` through it.
const MOOR_FETCH: &str = "+refs/heads/*:refs/remotes/moor/*";

/// Wall-clock allowance for one setup line.
pub const SETUP_LINE_DEADLINE: Duration = Duration::from_mins(30);
/// Wall-clock allowance for the teardown script.
pub const TEARDOWN_DEADLINE: Duration = Duration::from_mins(30);
/// Wall-clock allowance for one `moor run`.
pub const RUN_DEADLINE: Duration = Duration::from_hours(24);

/// Why a command runs in the Box; sets only the deadline and the pty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The setup script of `lines` lines.
    Setup {
        /// Number of setup lines; the deadline scales with it.
        lines: usize,
    },
    /// The teardown script.
    Teardown,
    /// A `moor run` command.
    Run,
    /// The agent session.
    Agent,
}

/// Where the Proxy CA was staged and which port the Box reaches the Proxy on.
#[derive(Debug, Clone, Copy)]
pub struct ProxyCtx<'a> {
    /// The Proxy port inside the Box.
    pub box_port: u16,
    /// The Box directory where [`stage_steps`] put the CA.
    pub cred: &'a BoxPath,
    /// Hosts the Box reaches without the Proxy.
    pub direct: &'a [DirectHost],
}

/// What the environment is built from. Per-Box home, xcconfig and Box tag variables for Mac Boxes
/// are added here, so every purpose gets them.
#[derive(Debug, Clone, Copy)]
pub struct BoxCtx<'a> {
    /// Repo `env:` already merged with the Proxy rule env (see [`merged_env`]).
    pub env: &'a [(String, String)],
    /// The Proxy, when the Box reaches the network through it.
    pub proxy: Option<ProxyCtx<'a>>,
    /// The git origin URL; drives the `moor` push remote lines.
    pub origin: Option<&'a str>,
    /// The directory commands run in; none keeps the driver's default directory.
    pub workdir: Option<&'a BoxPath>,
    /// Provider variable names removed from the Box environment.
    pub strip: &'a [String],
}

/// The environment for one Box exec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecEnv {
    /// Non-secret `NAME=value` pairs.
    pub vars: Vec<(String, String)>,
    /// Names to unset before the command runs (the strip names, deduplicated).
    pub unset: Vec<String>,
    /// Shell lines run before the command. Always starts with the image `PATH` line; the Proxy
    /// lines follow when there is a Proxy.
    pub prelude: Vec<String>,
    /// The working directory; none keeps the driver's default directory.
    pub cwd: Option<BoxPath>,
    /// The wall-clock limit, none for the agent.
    pub deadline: Option<Deadline>,
    /// Whether the exec gets a pty.
    pub pty: bool,
}

/// Builds the environment for `purpose`. Everything but the deadline and pty is the same for every
/// purpose.
///
/// # Errors
/// Returns a message when an env name is not a shell identifier or a credential path is invalid.
pub fn build(b: &BoxCtx<'_>, purpose: Purpose) -> Result<ExecEnv, String> {
    if let Some(bad) = b.env.iter().find(|(n, _)| !is_identifier(n)) {
        return Err(format!("{:?} is not a shell variable name", bad.0));
    }
    let mut unset: Vec<String> = Vec::new();
    for name in b.strip {
        if !is_identifier(name) {
            return Err(format!("{name:?} is not a shell variable name"));
        }
        if !unset.contains(name) {
            unset.push(name.clone());
        }
    }
    let mut prelude = vec![image_path_line()];
    if let Some(p) = &b.proxy {
        prelude.extend(proxy_lines(p)?);
    }
    if b.proxy.is_some() {
        prelude.extend(git_remote_lines(b.origin));
        prelude.push(format!(
            "unset moor_bundle moor_sys moor_mark moor_f {}",
            java::JAVA_SHELL_VARS
        ));
        // The agent launch script exports the repo env, these included, just before the agent starts.
        if !matches!(purpose, Purpose::Agent) {
            for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
                if let Some((_, v)) = b
                    .env
                    .iter()
                    .find(|(n, _)| n == name && !b.strip.contains(n))
                {
                    prelude.push(format!("export {name}={}", quote(v)));
                }
            }
        }
    }
    let (deadline, pty) = match purpose {
        Purpose::Setup { lines } => {
            let n = u32::try_from(lines).unwrap_or(u32::MAX);
            (Some(SETUP_LINE_DEADLINE.saturating_mul(n)), false)
        }
        Purpose::Teardown => (Some(TEARDOWN_DEADLINE), false),
        Purpose::Run => (Some(RUN_DEADLINE), false),
        Purpose::Agent => (None, true),
    };
    Ok(ExecEnv {
        vars: b
            .env
            .iter()
            .filter(|(n, _)| !b.strip.contains(n))
            .cloned()
            .collect(),
        unset,
        prelude,
        cwd: b.workdir.cloned(),
        deadline: deadline.map(Deadline),
        pty,
    })
}

/// The repo `env` followed by the rule `env`; a rule's value replaces the repo's of the same name.
#[must_use]
pub fn merged_env(
    repo: Vec<(String, String)>,
    rules: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = repo
        .into_iter()
        .filter(|(name, _)| !rules.iter().any(|(r, _)| r == name))
        .collect();
    out.extend(rules);
    out
}

/// `argv` wrapped so the unset line and the prelude run first.
///
/// [`build`] always yields a prelude, so `argv` is unchanged only for a hand-built [`ExecEnv`] with
/// nothing to unset, an empty prelude and no `cleanup`. Otherwise it becomes
/// `sh -c '<prelude>\nexec "$@"' sh argv...`. With `cleanup`, the script has no `exec`: it sets an
/// `EXIT` trap that removes the `cleanup` directory (the staged credentials), runs the command, and
/// exits with the command's status.
#[must_use]
pub fn wrap_argv(env: &ExecEnv, argv: Vec<String>, cleanup: Option<&BoxPath>) -> Vec<String> {
    if env.unset.is_empty() && env.prelude.is_empty() && cleanup.is_none() {
        return argv;
    }
    let mut lines = Vec::new();
    if let Some(dir) = cleanup {
        let rm = format!("rm -rf {}", quote(dir.as_str()));
        lines.push(format!("trap {} EXIT", quote(&rm)));
    }
    if !env.unset.is_empty() {
        lines.push(format!("unset {}", env.unset.join(" ")));
    }
    lines.extend(env.prelude.iter().cloned());
    lines.push(
        if cleanup.is_some() {
            r#""$@""#
        } else {
            r#"exec "$@""#
        }
        .to_owned(),
    );
    let mut out = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        lines.join("\n"),
        "sh".to_owned(),
    ];
    out.extend(argv);
    out
}

/// The step that stages the public Proxy CA certificate in `cred`, as `<cred>/proxy-ca.pem`
/// (mode 0644), with the text a failure of it is reported under.
///
/// # Errors
/// Returns a message when the credential path is invalid.
pub fn stage_steps(cred: &BoxPath, ca_pem: &str) -> Result<[(BoxStep, &'static str); 1], String> {
    let ca = BoxStep::Put {
        path: cred_file(cred, CA_FILE)?,
        bytes: ca_pem.as_bytes().to_vec(),
        mode: 0o644,
    };
    Ok([(ca, "could not stage the proxy CA")])
}

/// `https://github.com/<owner>/<name>.git` for a GitHub origin (`https://github.com/o/n(.git)`,
/// `ssh://git@github.com/o/n(.git)`, `git@github.com:o/n(.git)`); none for any other host or shape.
#[must_use]
pub fn push_url(origin: &str) -> Option<String> {
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
    let repo = (plain(owner) && plain(name)).then(|| ForgeRepo::new(owner, name))?;
    Some(format!(
        "https://github.com/{}/{}.git",
        repo.owner, repo.name
    ))
}

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `rel` under the credential directory.
fn cred_file(cred: &BoxPath, rel: &str) -> Result<BoxPath, String> {
    BoxPath::under(cred.as_str(), rel).map_err(|e| e.to_string())
}

/// Lines that trust the CA and export the Proxy variables.
fn proxy_lines(p: &ProxyCtx<'_>) -> Result<Vec<String>, String> {
    let ca = quote(cred_file(p.cred, CA_FILE)?.as_str());
    let built = quote(cred_file(p.cred, BUNDLE_FILE)?.as_str());
    let mut lines = vec![
        "unset GH_TOKEN GITHUB_TOKEN".to_owned(),
        "moor_sys=; for moor_f in ".to_owned()
            + SYSTEM_BUNDLES
            + "; do [ -f \"$moor_f\" ] && { moor_sys=$moor_f; break; }; done",
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
        lines.push(format!("export {name}=\"http://127.0.0.1:{}\"", p.box_port));
    }
    lines.extend(yarn_lines(p.box_port));
    let direct: String = p
        .direct
        .iter()
        .map(|h| [",", &h.no_proxy()].concat())
        .collect();
    for name in ["NO_PROXY", "no_proxy"] {
        lines.push(format!(
            "export {name}=\"127.0.0.1,localhost,::1{direct}${{{name}:+,${name}}}\""
        ));
    }
    lines.push("export NODE_USE_ENV_PROXY=1".to_owned());
    lines.extend(java::java_lines(p.box_port, &ca, p.direct));
    Ok(lines)
}

/// The line that appends the `PATH` of [`IMAGE_ENV_FILE`], when it sets one, to the exec's `PATH`.
fn image_path_line() -> String {
    image_path_line_for(IMAGE_ENV_FILE)
}

/// [`image_path_line`] reading `file`. The image entries go last, so a command the exec's `PATH`
/// already resolves keeps resolving to the same program; a Box without the file is unchanged.
fn image_path_line_for(file: &str) -> String {
    format!(
        "moor_img=$(sed -n 's/^PATH=//p' {} 2>/dev/null | tail -n 1 | tr -d \"\\\"'\"); [ -z \"$moor_img\" ] || PATH=\"$PATH:$moor_img\"; unset moor_img",
        quote(file)
    )
}

/// Shell lines that point Yarn and npm at the proxy and a cache the agent can write.
fn yarn_lines(box_port: u16) -> Vec<String> {
    let mut lines: Vec<String> = ["YARN_HTTPS_PROXY", "YARN_HTTP_PROXY"]
        .iter()
        .map(|name| format!("export {name}=\"http://127.0.0.1:{box_port}\""))
        .collect();
    lines.extend(
        [
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

mod java;

#[cfg(test)]
mod tests;
