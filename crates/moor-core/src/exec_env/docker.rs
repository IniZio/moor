//! The prelude line that points the Box user's Docker at the egress proxy. Docker reads
//! `proxies.default` in `~/.docker/config.json`, passes it to every `docker build` as build
//! arguments and to every container as environment variables.

use super::egress::DOCKER_SOURCES;

/// The address of Docker's default bridge on the Box. Containers on the default bridge and on
/// Compose networks reach a service of the Box here.
pub(super) const BRIDGE_GATEWAY: &str = "172.17.0.1";

/// The Box user's Docker client config, as shell text that expands on the Box.
const CONFIG: &str = "$HOME/.docker/config.json";

/// The `noProxy` value: the container itself and the networks Docker allocates, so containers talk
/// to each other direct.
fn no_proxy() -> String {
    format!("localhost,127.0.0.1,::1,{}", DOCKER_SOURCES.join(","))
}

/// The `proxies.default` value `jq` merges into the config.
const MERGE: &str =
    ".proxies.default = ((.proxies.default // {}) + {httpProxy: $p, httpsProxy: $p, noProxy: $n})";

/// The line that sets `proxies.default.httpProxy`, `httpsProxy` and `noProxy` to the egress proxy
/// on `port`, and leaves every other key of the file as it was.
///
/// A missing or empty file is written new, mode 0600. An existing file is merged with `jq`; the
/// merge is skipped when it changes nothing, and otherwise replaces the file by rename with the
/// file's own mode. A Box with no `jq`, or a config that is not a JSON object, keeps its file and
/// the line prints a warning to stderr. The line never stops the exec.
pub(super) fn line(port: u16) -> String {
    let url = format!("http://{BRIDGE_GATEWAY}:{port}");
    let jq = format!("jq --arg p \"$moor_dkp\" --arg n \"$moor_dkn\" '{MERGE}'");
    let warn = |why: &str| {
        format!("echo \"moor: docker proxy settings not written to {CONFIG}: {why}\" >&2")
    };
    format!(
        "moor_dk=\"{CONFIG}\"; moor_dkp=\"{url}\"; moor_dkn=\"{}\"; if [ -s \"$moor_dk\" ]; then if [ -L \"$moor_dk\" ] || [ ! -f \"$moor_dk\" ]; then {}; elif ! command -v jq >/dev/null 2>&1; then {}; elif {jq} \"$moor_dk\" 2>/dev/null | cmp -s - \"$moor_dk\"; then :; elif cp -p \"$moor_dk\" \"$moor_dk.$$\" && {jq} \"$moor_dk\" >\"$moor_dk.$$\" && mv -f \"$moor_dk.$$\" \"$moor_dk\"; then :; else rm -f \"$moor_dk.$$\"; {}; fi; else mkdir -p \"$HOME/.docker\" && printf '{{\\n  \"proxies\": {{\\n    \"default\": {{\\n      \"httpProxy\": \"%s\",\\n      \"httpsProxy\": \"%s\",\\n      \"noProxy\": \"%s\"\\n    }}\\n  }}\\n}}\\n' \"$moor_dkp\" \"$moor_dkp\" \"$moor_dkn\" >\"$moor_dk.$$\" && chmod 600 \"$moor_dk.$$\" && mv -f \"$moor_dk.$$\" \"$moor_dk\" || {{ rm -f \"$moor_dk.$$\"; {}; }}; fi; unset moor_dk moor_dkp moor_dkn",
        no_proxy(),
        warn("it is a symlink or not a regular file"),
        warn("the file exists and the Box has no jq to merge into it"),
        warn("the file is not a JSON object"),
        warn("the file could not be written"),
    )
}
