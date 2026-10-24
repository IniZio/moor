//! The prelude line that points the Box user's Docker client at the egress proxy.

use super::egress::DOCKER_SOURCES;

/// The Box user's Docker client config, as shell text that expands on the Box.
const CONFIG: &str = "$HOME/.docker/config.json";

/// The `noProxy` value: the container itself and the networks Docker allocates.
fn no_proxy() -> String {
    format!("localhost,127.0.0.1,::1,{}", DOCKER_SOURCES.join(","))
}

/// The `proxies.default` value `jq` merges into the config.
const MERGE: &str =
    ".proxies.default = ((.proxies.default // {}) + {httpProxy: $p, httpsProxy: $p, noProxy: $n})";

/// The line that merges the egress proxy at the `moor_br` address on `port` into
/// `proxies.default`, or does nothing while `moor_br` is empty. A file it cannot merge safely is
/// kept with a warning on stderr; the line never stops the exec.
pub(super) fn line(port: u16) -> String {
    let jq = format!("jq --arg p \"$moor_dkp\" --arg n \"$moor_dkn\" '{MERGE}'");
    let warn = |why: &str| {
        format!("echo \"moor: docker proxy settings not written to {CONFIG}: {why}\" >&2")
    };
    format!(
        "if [ -n \"$moor_br\" ]; then moor_dk=\"{CONFIG}\"; moor_dkp=\"http://${{moor_br%% *}}:{port}\"; moor_dkn=\"{}\"; if [ -L \"$moor_dk\" ] || {{ [ -e \"$moor_dk\" ] && [ ! -f \"$moor_dk\" ]; }}; then {}; elif [ -s \"$moor_dk\" ]; then if ! command -v jq >/dev/null 2>&1; then {}; elif {jq} \"$moor_dk\" 2>/dev/null | cmp -s - \"$moor_dk\"; then :; elif cp -p \"$moor_dk\" \"$moor_dk.$$\" && {jq} \"$moor_dk\" >\"$moor_dk.$$\" && mv -f \"$moor_dk.$$\" \"$moor_dk\"; then :; else rm -f \"$moor_dk.$$\"; {}; fi; else mkdir -p \"$HOME/.docker\" && printf '{{\\n  \"proxies\": {{\\n    \"default\": {{\\n      \"httpProxy\": \"%s\",\\n      \"httpsProxy\": \"%s\",\\n      \"noProxy\": \"%s\"\\n    }}\\n  }}\\n}}\\n' \"$moor_dkp\" \"$moor_dkp\" \"$moor_dkn\" >\"$moor_dk.$$\" && chmod 600 \"$moor_dk.$$\" && mv -f \"$moor_dk.$$\" \"$moor_dk\" || {{ rm -f \"$moor_dk.$$\"; {}; }}; fi; unset moor_dk moor_dkp moor_dkn; fi",
        no_proxy(),
        warn("it is a symlink or not a regular file"),
        warn("the file exists and the Box has no jq to merge into it"),
        warn("the file is not a JSON object"),
        warn("the file could not be written"),
    )
}
