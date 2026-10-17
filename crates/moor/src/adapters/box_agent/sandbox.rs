//! The nono sandbox the Box agent runs under: the check script, the profile and the wrapping argv.

use std::collections::BTreeMap;

use moor_core::exec_env;
use moor_core::name::BoxPath;
use serde_json::{json, Value};

/// The pinned nono release.
pub(super) const NONO_VERSION: &str = "0.80.0";
/// File name of the generated profile inside the credential directory.
pub(super) const PROFILE_FILE: &str = "nono-profile.json";

/// The variable that makes Claude Code accept bypassPermissions as root.
pub(super) const ROOT_SANDBOX_VAR: &str = "IS_SANDBOX";

/// Exit code of the check script when the Box lacks the pinned nono binary.
pub(super) const NOT_INSTALLED: i32 = 6;

/// Login and terminal names every agent keeps, plus two globs; the Proxy and CA names come from
/// `exec_env::PASS_VARS`.
const LOGIN_VARS: [&str; 15] = [
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "CFFIXED_USER_HOME",
    "XCODE_XCCONFIG_FILE",
    "MOOR_BOX_TAG",
    "DEVELOPER_DIR",
    "COLORTERM",
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
    /// The Box's operating system as `uname -s` reports it.
    pub(super) os: &'a str,
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
    /// The macOS Box's layout and grants; none on a Box that is not macOS or has no layout.
    pub(super) mac: Option<mac::MacInput<'a>>,
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
    let mut vars: Vec<String> = LOGIN_VARS
        .iter()
        .chain(exec_env::PASS_VARS)
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

/// The profile's `filesystem` section: agent state is the Task home's, toolchains are read-only.
fn filesystem(input: &SandboxInput) -> Value {
    let home = input.home.as_str();
    let at = |rel: &str| format!("{home}/{rel}");
    // The agent's binaries and toolchains live in the Box user's real home, which on a macOS Box
    // is not the Task home; they are only read, never written.
    let tools = input.mac.as_ref().map_or(home, |m| m.login_home.as_str());
    let tool = |rel: &str| format!("{tools}/{rel}");
    let mut allow = vec![
        input.workdir.as_str().to_owned(),
        input.tmp.as_str().to_owned(),
        at(".claude"),
        at(".cache"),
        at(".config/gh"),
        at(".local/state/claude"),
    ];
    let mut read = vec![
        input.cred.as_str().to_owned(),
        "/usr/local/bin".to_owned(),
        "/usr/local/lib".to_owned(),
        tool(".local/share/mise"),
        tool(".bun"),
        tool(".npm-global"),
    ];
    // Claude Code keeps its versions here; a macOS Box reads the login home's copy, and the
    // Task home gets no write grant for it.
    if input.mac.is_some() {
        read.push(tool(".local/share/claude"));
    } else {
        allow.push(at(".local/share/claude"));
    }
    json!({
        "unix_socket_dir": ["/run/systemd/userdb"],
        "allow": allow,
        "allow_file": [at(".claude.json")],
        "read": read,
        "read_file": ["/etc/gitconfig", tool(".gitconfig")],
    })
}

/// The nono profile as pretty JSON text.
///
/// # Errors
/// The macOS grants cannot be rendered into the profile.
pub(super) fn profile(input: &SandboxInput) -> Result<String, String> {
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
    let mut doc = json!({
        "meta": { "name": "moor-agent" },
        "groups": { "include": DEFAULT_GROUPS },
        "security": { "signal_mode": "isolated" },
        "linux": { "af_unix_mediation": "pathname" },
        "workdir": { "access": "readwrite" },
        "filesystem": filesystem(input),
        "environment": {
            "allow_vars": allow_vars(&input.env_names, input.strip_env),
            "set_vars": set_vars,
        },
    });
    if input.os != "Linux" {
        // Only Linux has pathname socket mediation and a userdb socket to reach.
        if let Some(d) = doc.as_object_mut() {
            d.shift_remove("linux");
            if let Some(f) = d.get_mut("filesystem").and_then(|f| f.as_object_mut()) {
                f.shift_remove("unix_socket_dir");
            }
        }
    }
    if let Some(m) = &input.mac {
        mac::apply(&mut doc, m)?;
    }
    Ok(serde_json::to_string_pretty(&doc).unwrap_or_default())
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
pub(super) fn check_failure(os: &str, code: i32) -> String {
    let v = NONO_VERSION;
    match code {
        5 => {
            let missing = if os == "Darwin" {
                "Seatbelt is unavailable"
            } else {
                "Landlock is missing"
            };
            format!(
                "the Box kernel cannot run the nono sandbox ({missing}); \
                 `nono setup --check-only` failed"
            )
        }
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

/// Shell variable holding the PID namespace prefix, empty unless the agent runs as root.
pub(super) const NAMESPACE_VAR: &str = "moor_ns";

/// Shell lines that set `$moor_ns` to an `unshare` prefix when the Box user is root, so the
/// agent gets its own PID namespace and a private `/proc`: nono grants the whole of `/proc`,
/// and for root only the kernel's ptrace check would then guard other processes' `environ`.
/// Root without an `unshare` that supports `--kill-child` stops the launch.
pub(super) fn namespace_lines() -> Vec<String> {
    let var = NAMESPACE_VAR;
    let refusal = "moor: the agent runs as root and needs unshare --kill-child (util-linux 2.36 or newer) on the Box to hide other processes";
    vec![format!(
        "{var}=; if [ \"$(id -u)\" = 0 ]; then {{ command -v unshare >/dev/null 2>&1 && unshare --help 2>&1 | grep -q -- --kill-child; }} || {{ echo '{refusal}' >&2; exit 1; }}; {var}='unshare --pid --fork --kill-child --mount-proc --'; fi"
    )]
}

/// The argv that starts the sandbox tool at `launcher` with the profile, ending in `--`; with
/// `report` the tool prints its session diagnostics as JSON on stderr.
pub(super) fn prefix(launcher: &BoxPath, profile_path: &BoxPath, report: bool) -> Vec<String> {
    [launcher.as_str(), "-s", "run"]
        .into_iter()
        .chain(report.then_some("--diagnostics-json"))
        .chain(["--profile", profile_path.as_str(), "--"])
        .map(str::to_owned)
        .collect()
}

/// The argv that runs `agent` under the sandbox tool at `launcher` with the profile.
pub(super) fn wrap(launcher: &BoxPath, profile_path: &BoxPath, agent: &[String]) -> Vec<String> {
    let mut argv = prefix(launcher, profile_path, false);
    argv.extend(agent.iter().cloned());
    argv
}

pub(super) mod mac;

#[cfg(test)]
mod tests;
