//! The nono sandbox the Box agent runs under: the check script, the profile and the wrapping argv.

use std::collections::BTreeMap;

use moor_core::name::BoxPath;
use serde_json::json;

/// The pinned nono release.
pub(super) const NONO_VERSION: &str = "0.80.0";
/// File name of the generated profile inside the credential directory.
pub(super) const PROFILE_FILE: &str = "nono-profile.json";

/// The variable that makes Claude Code accept bypassPermissions as root.
pub(super) const ROOT_SANDBOX_VAR: &str = "IS_SANDBOX";

/// Exit code of the check script when the Box lacks the pinned nono binary.
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
    /// Variables the sandbox sets for the agent alone: repo `env` and rule `box_env`.
    pub(super) set_vars: Vec<(String, String)>,
    /// The Box user is root; the sandbox then tells Claude Code it is sandboxed.
    pub(super) root: bool,
    /// Names that never reach the sandbox, whatever the repo or rules ask for.
    pub(super) strip_env: &'a [String],
}

/// True when `name` may be passed into the sandbox.
fn passable(name: &str, strip_env: &[String]) -> bool {
    !name.is_empty()
        && !name.starts_with("NSC_")
        && !name.eq_ignore_ascii_case("token")
        && !GATE_VARS.contains(&name)
        && !strip_env.iter().any(|s| s == name)
}

/// The sorted, deduplicated environment allow list.
fn allow_vars(env_names: &[String], strip_env: &[String]) -> Vec<String> {
    let mut vars: Vec<String> = BASE_VARS
        .iter()
        .map(|n| (*n).to_owned())
        .chain(env_names.iter().filter(|n| passable(n, strip_env)).cloned())
        .collect();
    vars.sort();
    vars.dedup();
    vars
}

/// The groups the built-in `default` profile includes. The profile spells them out instead of
/// extending `default`, because a user profile of that name in the Box would replace the built-in.
const DEFAULT_GROUPS: [&str; 18] = [
    "deny_credentials",
    "deny_keychains_macos",
    "deny_keychains_linux",
    "deny_browser_data_macos",
    "deny_browser_data_linux",
    "deny_macos_private",
    "deny_shell_history",
    "deny_shell_configs",
    "system_read_macos",
    "system_read_linux_core",
    "system_write_macos",
    "system_write_linux",
    "user_tools",
    "homebrew_macos",
    "homebrew_linux",
    "dangerous_commands",
    "dangerous_commands_macos",
    "dangerous_commands_linux",
];

/// The nono profile as pretty JSON text.
pub(super) fn profile(input: &SandboxInput) -> String {
    let home = input.home.as_str();
    let at = |rel: &str| format!("{home}/{rel}");
    let mut set_vars: BTreeMap<_, _> = input
        .set_vars
        .iter()
        .filter(|(n, _)| !input.strip_env.contains(n))
        .cloned()
        .collect();
    if input.root {
        // Claude Code refuses bypassPermissions as root unless it is told it is sandboxed.
        set_vars.insert(ROOT_SANDBOX_VAR.to_owned(), "1".to_owned());
    }
    let doc = json!({
        "meta": { "name": "moor-agent" },
        "groups": { "include": DEFAULT_GROUPS },
        "security": { "signal_mode": "isolated" },
        "linux": { "af_unix_mediation": "pathname" },
        "workdir": { "access": "readwrite" },
        "filesystem": {
            "unix_socket_dir": ["/run/systemd/userdb"],
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
        "environment": {
            "allow_vars": allow_vars(&input.env_names, input.strip_env),
            "set_vars": set_vars,
        },
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// The POSIX shell script that checks the Box's nono is the pinned binary, with sha256 `sha256`,
/// and can run; run via `sh -c`.
pub(super) fn check_script(sha256: &str) -> String {
    let (v, missing) = (NONO_VERSION, NOT_INSTALLED);
    format!(
        r#"dest="$HOME/.local/bin/nono"
sum=$(sha256sum "$dest" 2>/dev/null || shasum -a 256 "$dest" 2>/dev/null)
[ "${{sum%% *}}" = "{sha256}" ] || exit {missing}
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
        NOT_INSTALLED => format!("nono {v} on the Box is missing or is not the pinned binary"),
        _ => format!("checking nono {v} exited {code}"),
    }
}

/// Shell lines run before the sandbox starts: no `NONO_*` setting from the environment reaches it, and
/// the profile lookup is pinned to the user's own config directory, which the agent cannot write.
pub(super) fn harden_lines() -> Vec<String> {
    vec![
        "for moor_v in $(env | sed -n 's/^\\(NONO_[A-Za-z0-9_]*\\)=.*/\\1/p'); do unset \"$moor_v\"; done; unset moor_v"
            .to_owned(),
        "export XDG_CONFIG_HOME=\"$HOME/.config\"".to_owned(),
    ]
}

/// The argv that runs `agent` under the sandbox tool at `launcher` with the profile.
pub(super) fn wrap(launcher: &BoxPath, profile_path: &BoxPath, agent: &[String]) -> Vec<String> {
    [launcher.as_str(), "-s", "run", "--profile"]
        .into_iter()
        .map(str::to_owned)
        .chain([profile_path.as_str().to_owned(), "--".to_owned()])
        .chain(agent.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests;
