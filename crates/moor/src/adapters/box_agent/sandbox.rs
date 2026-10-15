//! The nono sandbox the Box agent runs under: the install script, the profile and the wrapping argv.

use moor_core::name::BoxPath;
use serde_json::json;

/// The pinned nono release.
pub(super) const NONO_VERSION: &str = "0.80.0";
/// File name of the generated profile inside the credential directory.
pub(super) const PROFILE_FILE: &str = "nono-profile.json";

/// Release download base URL.
const RELEASE_URL: &str = "https://github.com/nolabs-ai/nono/releases/download/v0.80.0";
/// Each supported platform: `uname -s`, `uname -m` pattern, asset and sha256.
const ASSETS: [(&str, &str, &str, &str); 4] = [
    (
        "Linux",
        "x86_64",
        "nono-v0.80.0-x86_64-unknown-linux-gnu.tar.gz",
        "6da665b61999570c10dffc7da78828fe1fb7f3cac53319e505cebecaef17037f",
    ),
    (
        "Linux",
        "aarch64|arm64",
        "nono-v0.80.0-aarch64-unknown-linux-gnu.tar.gz",
        "d2a24c8c37714f85c829b5586ffd81fc2acf336c95372d881ba62140884f22cb",
    ),
    (
        "Darwin",
        "x86_64",
        "nono-v0.80.0-x86_64-apple-darwin.tar.gz",
        "b354e674e6b4643ad5c572d148ae367bb0c5c103410fbc22923c5593a6f7d0b2",
    ),
    (
        "Darwin",
        "arm64",
        "nono-v0.80.0-aarch64-apple-darwin.tar.gz",
        "ae824167b3d90531b830429ba92b7781379b1c2d79f9b11fdd88b0fd267e4e32",
    ),
];

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
                at(".claude"),
                at(".cache"),
                at(".config/gh"),
                at(".local/state"),
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

/// The shell `case` arms that map the platform to its asset and sha256.
fn platform_arms() -> String {
    ASSETS
        .iter()
        .map(|(os, arch, asset, sum)| format!("  {os}:{arch}) asset={asset}; sum={sum} ;;"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The POSIX shell script that installs the pinned nono for the Box user; run via `sh -c`.
pub(super) fn install_script() -> String {
    let arms = platform_arms();
    let v = NONO_VERSION;
    format!(
        r#"dest="$HOME/.local/bin/nono"
if [ "$("$dest" --version 2>/dev/null)" != "nono {v}" ]; then
  case "$(uname -s):$(uname -m)" in
{arms}
  *) echo "no nono {v} release for $(uname -s) $(uname -m)" >&2; exit 2 ;;
  esac
  tmp="$(mktemp -d)" || exit 4
  trap 'rm -rf "$tmp"' EXIT
  url="{RELEASE_URL}/$asset"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$tmp/$asset" "$url" || exit 4
  else
    wget -qO "$tmp/$asset" "$url" || exit 4
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && echo "$sum  $asset" | sha256sum -c - >/dev/null 2>&1) || exit 3
  else
    (cd "$tmp" && echo "$sum  $asset" | shasum -a 256 -c - >/dev/null 2>&1) || exit 3
  fi
  tar -xzf "$tmp/$asset" -C "$tmp" nono || exit 4
  mkdir -p "$(dirname "$dest")" || exit 4
  install -m 755 "$tmp/nono" "$dest" || exit 4
fi
"$dest" setup --check-only >/dev/null 2>&1 || exit 5
"#
    )
}

/// A human message for a non-zero exit of the install script.
pub(super) fn install_failure(code: i32) -> String {
    let v = NONO_VERSION;
    match code {
        2 => format!("nono {v} has no release for this Box's OS and CPU"),
        3 => format!("the nono {v} download failed its sha256 check"),
        4 => format!("could not download nono {v} into the Box"),
        5 => "the Box kernel cannot run the nono sandbox (Landlock is missing); \
              `nono setup --check-only` failed"
            .to_owned(),
        _ => format!("installing nono {v} exited {code}"),
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
