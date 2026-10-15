//! The nono sandbox the Box agent runs under: the check script, the profile and the wrapping argv.

use moor_core::name::BoxPath;
use serde_json::json;

/// The pinned nono release.
pub(super) const NONO_VERSION: &str = "0.80.0";
/// File name of the generated profile inside the credential directory.
pub(super) const PROFILE_FILE: &str = "nono-profile.json";

/// Exit code of the check script when the Box lacks the pinned nono.
pub(super) const NOT_INSTALLED: i32 = 6;

/// Environment names every agent keeps: terminal, proxy and CA variables, plus two globs.
const BASE_VARS: [&str; 23] = [
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "COLORTERM",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "https_proxy",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "NODE_USE_ENV_PROXY",
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "NODE_EXTRA_CA_CERTS",
    "GIT_CONFIG_*",
    "CLAUDE_*",
];

/// Names that are never passed into the sandbox: the crabbox gate variables.
const GATE_VARS: [&str; 8] = [
    "child",
    "child_identity",
    "child_pid",
    "command_umask",
    "gate",
    "run_dir",
    "state",
    "token",
];

/// What the profile is built from.
pub(super) struct SandboxInput<'a> {
    /// Proxy tunnel port on the Box.
    pub(super) box_port: u16,
    /// Project working directory.
    pub(super) workdir: &'a BoxPath,
    /// The Box user's home.
    pub(super) home: &'a BoxPath,
    /// Credential directory holding the CA files; granted read-only.
    pub(super) cred: &'a BoxPath,
    /// Per-Task temp directory; granted read-write.
    pub(super) tmp: &'a BoxPath,
    /// Extra names to pass: agent env and rule `box_env` names.
    pub(super) env_names: Vec<String>,
}

/// True when `name` may be passed into the sandbox.
fn passable(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with("NSC_")
        && !name.eq_ignore_ascii_case("token")
        && !GATE_VARS.contains(&name)
}

/// The sorted, deduplicated environment allow list.
fn allow_vars(env_names: &[String]) -> Vec<String> {
    let mut vars: Vec<String> = BASE_VARS
        .iter()
        .map(|n| (*n).to_owned())
        .chain(env_names.iter().filter(|n| passable(n)).cloned())
        .collect();
    vars.sort();
    vars.dedup();
    vars
}

/// The nono profile as pretty JSON text.
pub(super) fn profile(input: &SandboxInput) -> String {
    let home = input.home.as_str();
    let at = |rel: &str| format!("{home}/{rel}");
    let doc = json!({
        "extends": "default",
        "meta": { "name": "moor-agent" },
        "linux": { "af_unix_mediation": "pathname" },
        "network": { "block": true, "open_port": [input.box_port] },
        "workdir": { "access": "readwrite" },
        "filesystem": {
            "allow": [
                input.workdir.as_str(),
                input.tmp.as_str(),
                at(".claude"),
                at(".cache"),
                at(".config/gh"),
                at(".local/state/claude"),
                at(".local/share/claude"),
            ],
            "allow_file": [at(".claude.json")],
            "read": [
                input.cred.as_str(),
                "/usr/local/bin",
                "/usr/local/lib",
                at(".local/share/mise"),
                at(".bun"),
                at(".npm-global"),
            ],
            "read_file": ["/etc/gitconfig", at(".gitconfig")],
        },
        "environment": { "allow_vars": allow_vars(&input.env_names) },
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// The POSIX shell script that checks the pinned nono is installed and can run; run via `sh -c`.
pub(super) fn check_script() -> String {
    let (v, missing) = (NONO_VERSION, NOT_INSTALLED);
    format!(
        r#"dest="$HOME/.local/bin/nono"
[ "$("$dest" --version 2>/dev/null)" = "nono {v}" ] || exit {missing}
"$dest" setup --check-only >/dev/null 2>&1 || exit 5
"#
    )
}

/// A human message for a non-zero exit of the check script.
pub(super) fn check_failure(code: i32) -> String {
    let v = NONO_VERSION;
    match code {
        5 => "the Box kernel cannot run the nono sandbox (Landlock is missing); \
              `nono setup --check-only` failed"
            .to_owned(),
        NOT_INSTALLED => format!("nono {v} is not installed on the Box"),
        _ => format!("checking nono {v} exited {code}"),
    }
}

/// The argv that runs `agent` under nono with the profile and the Proxy port.
pub(super) fn wrap(profile_path: &BoxPath, box_port: u16, agent: &[String]) -> Vec<String> {
    ["nono", "-s", "run", "--profile"]
        .into_iter()
        .map(str::to_owned)
        .chain([
            profile_path.as_str().to_owned(),
            "--block-net".to_owned(),
            "--open-port".to_owned(),
            box_port.to_string(),
            "--".to_owned(),
        ])
        .chain(agent.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests;
