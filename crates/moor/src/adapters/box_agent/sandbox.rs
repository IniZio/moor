//! The nono sandbox the Box agent runs under: the check script, the profile and the wrapping argv.

use std::collections::BTreeMap;

use moor_core::name::BoxPath;
use serde_json::json;

use super::device::Device;

/// The pinned nono release.
pub(super) const NONO_VERSION: &str = "0.80.0";
/// File name of the generated profile inside the credential directory.
pub(crate) const PROFILE_FILE: &str = "nono-profile.json";

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
pub(crate) struct SandboxInput<'a> {
    /// The Box's operating system as `uname -s` reports it.
    pub(crate) os: &'a str,
    /// Project working directory.
    pub(crate) workdir: &'a BoxPath,
    /// The agent's HOME: a per-Task directory on macOS, the Box user's home on Linux.
    pub(crate) home: &'a BoxPath,
    /// The Box login user's real home. Equals `home` on Linux. On macOS it holds credentials, so
    /// only toolchain paths under it are granted, read-only.
    pub(crate) login_home: &'a BoxPath,
    /// Credential directory holding the CA files; granted read-only.
    pub(crate) cred: &'a BoxPath,
    /// Per-Task temp directory; granted read-write.
    pub(crate) tmp: &'a BoxPath,
    /// Extra names to pass: agent env and rule `box_env` names.
    pub(crate) env_names: Vec<String>,
    /// Variables the sandbox sets for the agent alone: repo `env` and rule `box_env`.
    pub(crate) set_vars: Vec<(String, String)>,
    /// Host-resolved paths of the controller's own state and config, denied to the agent on
    /// macOS. May be empty; ignored on Linux, where Landlock cannot deny.
    pub(crate) moor_paths: &'a [String],
    /// Devices of this Task; each kind available on the Box's os gets its grants and env.
    pub(crate) devices: &'a [Device],
}

/// For each device whose kind the Box's os can host, grants its paths read-write and sets its
/// variables. Must run before the deny list is computed so no deny entry overrides a grant.
fn grant_devices(doc: &mut serde_json::Value, input: &SandboxInput) {
    for d in input.devices.iter().filter(|d| d.kind.available(input.os)) {
        if let Some(a) = doc
            .pointer_mut("/filesystem/allow")
            .and_then(|a| a.as_array_mut())
        {
            a.extend(
                d.kind
                    .grants(&d.id, input.login_home.as_str())
                    .into_iter()
                    .map(|p| json!(p)),
            );
        }
        if let Some(v) = doc
            .pointer_mut("/environment/set_vars")
            .and_then(|v| v.as_object_mut())
        {
            for (k, val) in d.kind.env(&d.id) {
                v.insert(k, json!(val));
            }
        }
    }
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

/// The read-only directories and files the profile grants.
fn read_grants(input: &SandboxInput) -> (Vec<String>, Vec<String>) {
    let mut read = vec![
        input.cred.as_str().to_owned(),
        "/usr/local/bin".to_owned(),
        "/usr/local/lib".to_owned(),
    ];
    let mut read_file = vec!["/etc/gitconfig".to_owned()];
    let (home, login) = (input.home.as_str(), input.login_home.as_str());
    if input.os == "Darwin" {
        // The login home is never granted wholesale or writable: it holds the owner's gh token,
        // ssh keys and docker auth. Only toolchains are readable from it.
        let under = |rel: &str| format!("{login}/{rel}");
        read.extend(
            [
                ".local/share/mise",
                ".bun",
                ".npm-global",
                ".local/bin",
                ".local/share/claude",
                "Library/Developer/Xcode",
            ]
            .map(under),
        );
        read.push("/Applications/Xcode.app".to_owned());
        read_file.push(under(".gitconfig"));
    } else {
        let under = |rel: &str| format!("{home}/{rel}");
        read.extend([".local/share/mise", ".bun", ".npm-global"].map(under));
        read_file.push(under(".gitconfig"));
    }
    (read, read_file)
}

/// Every path the profile grants, whatever the access.
fn granted(doc: &serde_json::Value) -> Vec<String> {
    ["allow", "allow_file", "read", "read_file"]
        .iter()
        .filter_map(|k| doc.pointer(&format!("/filesystem/{k}"))?.as_array())
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// The macOS deny list: the controller's state and config under the login home plus the host
/// `moor_paths`. A path equal to or containing a grant is skipped: Seatbelt would let the deny
/// override that grant and break the agent.
fn deny_paths(input: &SandboxInput, grants: &[String]) -> Vec<String> {
    let login = input.login_home.as_str();
    let defaults = [
        ".local/state/moor",
        ".config/moor",
        ".config/agent37",
        ".config/crabbox",
    ]
    .map(|rel| format!("{login}/{rel}"));
    let extra = input
        .moor_paths
        .iter()
        .filter(|p| p.starts_with('/'))
        .map(|p| p.trim_end_matches('/').to_owned())
        .filter(|p| !p.is_empty());
    let covers = |deny: &str| {
        grants
            .iter()
            .any(|g| g == deny || g.strip_prefix(deny).is_some_and(|r| r.starts_with('/')))
    };
    let mut deny: Vec<String> = defaults
        .into_iter()
        .chain(extra)
        .filter(|p| !covers(p))
        .collect();
    deny.sort();
    deny.dedup();
    deny
}

/// The nono profile as pretty JSON text.
pub(crate) fn profile(input: &SandboxInput) -> String {
    let home = input.home.as_str();
    let at = |rel: &str| format!("{home}/{rel}");
    let (read, read_file) = read_grants(input);
    let mut set_vars: BTreeMap<String, String> = input.set_vars.iter().cloned().collect();
    set_vars.insert("IS_SANDBOX".to_owned(), "1".to_owned());
    let mut doc = json!({
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
            "read": read,
            "read_file": read_file,
        },
        "environment": {
            "allow_vars": allow_vars(&input.env_names),
            "set_vars": set_vars,
        },
    });
    grant_devices(&mut doc, input);
    if input.os == "Darwin" {
        // Seatbelt's `(allow network-outbound)` also permits connecting to any unix socket, so
        // the Docker socket would be a root-equivalent escape. Blocking the network leaves
        // loopback TCP, which reaches the Proxy, and the DNS socket nono keeps open.
        let deny = deny_paths(input, &granted(&doc));
        if let Some(d) = doc.as_object_mut() {
            d.insert(
                "network".to_owned(),
                json!({ "block": true, "open_port": [0] }),
            );
            if let Some(f) = d.get_mut("filesystem").and_then(|f| f.as_object_mut()) {
                f.insert("deny".to_owned(), json!(deny));
            }
        }
    }
    if input.os != "Linux" {
        // Only Linux has pathname socket mediation and a userdb socket to reach.
        if let Some(d) = doc.as_object_mut() {
            d.shift_remove("linux");
            if let Some(f) = d.get_mut("filesystem").and_then(|f| f.as_object_mut()) {
                f.shift_remove("unix_socket_dir");
            }
        }
    }
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
