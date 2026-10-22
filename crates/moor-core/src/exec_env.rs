//! The one Box environment every exec uses: the same variables for every purpose, only the
//! deadline and pty differ.

use std::time::Duration;

use crate::boxscript::quote;
use crate::name::BoxPath;
use crate::seam::proc::Deadline;

/// The Box directory the prelude writes the public Proxy CA into, as shell text that expands on the Box.
const PROXY_DIR: &str = "$HOME/.moor-proxy";
/// The public Proxy CA certificate in [`PROXY_DIR`].
const CA_FILE: &str = "ca.pem";
/// The CA bundle built in [`PROXY_DIR`] when the system bundle cannot be edited.
const BUNDLE_FILE: &str = "ca-bundle.pem";
/// The start of the line, after `moor: ` and before the path, that the prelude prints to stderr
/// when it stops the exec because the CA or the bundle could not be written.
pub const CA_WRITE_FAILED: &str = "the Proxy CA could not be written to";
/// System CA bundles, first existing one wins.
const SYSTEM_BUNDLES: &str =
    "/etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem";
/// Where an image can record the `PATH` its Dockerfile `ENV` sets; non-interactive ssh execs do
/// not inherit that `ENV`.
const IMAGE_ENV_FILE: &str = "/etc/environment";

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
    /// The clone of the Box's repo.
    Clone,
    /// The teardown script.
    Teardown,
    /// A `moor run` command.
    Run,
    /// The agent session.
    Agent,
}

/// The Proxy CA the prelude writes and the port the Box reaches the Proxy on.
#[derive(Debug, Clone, Copy)]
pub struct ProxyCtx<'a> {
    /// The Proxy port inside the Box.
    pub box_port: u16,
    /// The public Proxy CA certificate in PEM form.
    pub ca_pem: &'a str,
}

/// What the environment is built from. Per-Box home, xcconfig and Box tag variables for Mac Boxes
/// are added here, so every purpose gets them.
#[derive(Debug, Clone, Copy)]
pub struct BoxCtx<'a> {
    /// Repo `env:` already merged with the Proxy rule env (see [`merged_env`]).
    pub env: &'a [(String, String)],
    /// The Proxy, when the Box reaches the network through it.
    pub proxy: Option<ProxyCtx<'a>>,
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
/// Returns a message when an env name is not a shell identifier.
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
        prelude.extend(proxy_lines(p));
    }
    if b.proxy.is_some() {
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
        Purpose::Clone => (Some(SETUP_LINE_DEADLINE), false),
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
/// nothing to unset and an empty prelude. Otherwise it becomes
/// `sh -c '<prelude>\nexec "$@"' sh argv...`.
#[must_use]
pub fn wrap_argv(env: &ExecEnv, argv: Vec<String>) -> Vec<String> {
    if env.unset.is_empty() && env.prelude.is_empty() {
        return argv;
    }
    let mut lines = Vec::new();
    if !env.unset.is_empty() {
        lines.push(format!("unset {}", env.unset.join(" ")));
    }
    lines.extend(env.prelude.iter().cloned());
    lines.push(r#"exec "$@""#.to_owned());
    let mut out = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        lines.join("\n"),
        "sh".to_owned(),
    ];
    out.extend(argv);
    out
}

/// True when `name` is a shell identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Shell that leaves the output of `write` in `file`, mode 0644, and fails when it cannot.
///
/// A `file` that is a symlink or not a regular file is refused. A `file` that already holds the
/// output is left alone, so it needs no free disk. Otherwise the output goes to the temp file
/// `file.$$`, which never outlives the line, and is renamed over `file`. The rename is atomic, so
/// an exec running at the same time reads either the old or the new file.
fn write_if_changed(write: &str, file: &str) -> String {
    let (file, tmp) = (format!("\"{file}\""), format!("\"{file}.$$\""));
    format!(
        "{{ if [ -L {file} ] || {{ [ -e {file} ] && [ ! -f {file} ]; }}; then false; elif {write} | cmp -s - {file}; then :; elif {write} >{tmp} && chmod 644 {tmp} && mv -f {tmp} {file}; then :; else rm -f {tmp}; false; fi; }}"
    )
}

/// `line`, or else the message naming `file` on stderr and the end of the exec.
fn or_stop(line: &str, file: &str) -> String {
    format!("{line} || {{ echo \"moor: {CA_WRITE_FAILED} {file}\" >&2; exit 1; }}")
}

/// Lines that write the CA, trust it and export the Proxy variables. When the CA or the bundle
/// cannot be written, the exec stops before the command with a message naming the file.
fn proxy_lines(p: &ProxyCtx<'_>) -> Vec<String> {
    let ca_file = format!("{PROXY_DIR}/{CA_FILE}");
    let ca = format!("\"{ca_file}\"");
    let built_file = format!("{PROXY_DIR}/{BUNDLE_FILE}");
    let mut lines = vec![
        "unset GH_TOKEN GITHUB_TOKEN".to_owned(),
        or_stop(
            &format!(
                "moor_ca={}; mkdir -p \"{PROXY_DIR}\" && {}",
                quote(p.ca_pem),
                write_if_changed("printf '%s' \"$moor_ca\"", &ca_file)
            ),
            &ca_file,
        ),
        "moor_sys=; for moor_f in ".to_owned()
            + SYSTEM_BUNDLES
            + "; do [ -f \"$moor_f\" ] && { moor_sys=$moor_f; break; }; done",
        format!("moor_mark=$(sed -n 2p {ca})"),
        format!(
            "if [ -n \"$moor_sys\" ] && sudo -n true 2>/dev/null && {{ grep -qF \"$moor_mark\" \"$moor_sys\" || sudo -n tee -a \"$moor_sys\" <{ca} >/dev/null; }}; then moor_bundle=$moor_sys; else moor_bundle=\"{built_file}\"; {}; fi",
            or_stop(
                &write_if_changed(
                    &format!("{{ if [ -n \"$moor_sys\" ]; then cat \"$moor_sys\"; fi; cat {ca}; }}"),
                    "$moor_bundle"
                ),
                &built_file,
            )
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
    for name in ["NO_PROXY", "no_proxy"] {
        lines.push(format!(
            "export {name}=\"127.0.0.1,localhost,::1${{{name}:+,${name}}}\""
        ));
    }
    lines.push("export NODE_USE_ENV_PROXY=1".to_owned());
    lines.extend(java::java_lines(p.box_port, &ca));
    lines
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

mod java;

#[cfg(test)]
mod tests;
