//! herdr-core holds the agent's terminal on the Box, so a dropped link leaves the agent running
//! and a new pane can attach to it: the Box commands that drive it.

use std::fmt::Write as _;

use moor_core::boxscript::quote;
use moor_core::seam::agent_adapter::AgentInstall;

use super::AGENT_PATH_LINE;

/// The name of the agent's terminal; a Box runs one agent at a time.
pub(in crate::adapters::box_agent) const SESSION: &str = "moor-agent";

/// The Box program that holds the agent's terminal, herdr-core called by its own name: `hc` is
/// also herbstclient's. The start refuses to run the agent without it.
pub(in crate::adapters::box_agent) const PROGRAM: &str = "herdr-core";

/// Replaces any agent terminal with a detached one running the script file `script`; exits the
/// shell when the terminal did not start. The terminal has no parent, so no other hc terminal's
/// end takes it down, and is kept once it ends, so its exit code can be read.
pub(super) fn open(script: &str) -> String {
    format!(
        "{PROGRAM} kill {SESSION} >/dev/null 2>&1; {PROGRAM} rm {SESSION} >/dev/null 2>&1; \
         {PROGRAM} run --bg --no-parent --name {SESSION} sh {} >/dev/null || exit 1;",
        quote(script)
    )
}

/// Exits with the ended agent's exit code: 1 when it left none, as after a signal, and 1 too
/// while it still runs or when there is no terminal.
pub(super) fn exit_code() -> String {
    format!(
        "c=$({PROGRAM} get {SESSION} --json 2>/dev/null | sed -n 's/.*\"exit_code\":\\([0-9][0-9]*\\).*/\\1/p'); exit \"${{c:-1}}\""
    )
}

/// Exits 0 only while the agent's terminal is live.
pub(super) fn alive() -> String {
    format!("{PROGRAM} get {SESSION} --json 2>/dev/null | grep -q '\"state\":\"live\"'")
}

/// Puts the calling terminal on the agent's terminal until the terminal ends.
pub(super) fn attach() -> String {
    format!("{PROGRAM} attach {SESSION}")
}

/// What a relayed client execs.
pub(super) fn relay() -> String {
    format!("exec {PROGRAM} attach {SESSION}")
}

/// Ends the agent's terminal, which hangs the agent up.
pub(super) fn kill() -> String {
    format!("{PROGRAM} kill {SESSION} >/dev/null")
}

/// The pinned herdr-core release.
const HC_VERSION: &str = "0.1.16";

/// One herdr-core release file a Box can run.
pub(super) struct Build {
    /// `uname -s uname -m` patterns of the Boxes it runs on, as a `case` pattern.
    pub(super) platform: &'static str,
    /// Its URL.
    pub(super) url: &'static str,
    /// Lowercase hex sha256 of the file.
    pub(super) sha256: &'static str,
}

/// The release files of [`HC_VERSION`], from herdr-core's release manifest.
const HC_BUILDS: [Build; 2] = [
    Build {
        platform: "'Linux x86_64'|'Linux amd64'",
        url: "https://releases.herdr.dev/core/0.1.16/herdr-core-linux-x86_64.tar.gz",
        sha256: "58d6861176dc6fe905b58880d5bff07a9959be613a98e5d66b4583b04b6cfa45",
    },
    Build {
        platform: "'Linux aarch64'|'Linux arm64'",
        url: "https://releases.herdr.dev/core/0.1.16/herdr-core-linux-aarch64.tar.gz",
        sha256: "b6573137d70baff24fb346c957a9879af6bdc4c641188cdf9e47a7b2a677bc92",
    },
];

/// The directory that holds the installed versions, one directory per version.
const HC_DIR: &str = "~/.local/share/moor/herdr-core";

/// The install of the pinned herdr-core.
fn hc_install() -> AgentInstall {
    AgentInstall {
        script: hc_install_script(&HC_BUILDS),
        present: format!("{AGENT_PATH_LINE}\n{PROGRAM} --version >/dev/null 2>&1"),
        dirs: vec![HC_DIR.to_owned()],
        check: format!(
            "for f in \"$HOME\"/{}/*/{PROGRAM}; do [ ! -e \"$f\" ] || \"$f\" --version >/dev/null 2>&1 || exit 1; done",
            HC_DIR.trim_start_matches("~/")
        ),
    }
}

/// The script that installs herdr-core from `builds` when the Box has none that runs. The
/// download is checked against its pinned sha256 before it is unpacked; a Box that restored the
/// install directory is linked to it and downloads nothing. `hc` is linked too unless the Box
/// has one already.
fn hc_install_script(builds: &[Build]) -> String {
    let dir = format!("\"$HOME\"/{}/{HC_VERSION}", HC_DIR.trim_start_matches("~/"));
    let fail = |what: &str| {
        format!(
            "{{ echo {} >&2; exit 1; }}",
            quote(&format!("moor: {what}"))
        )
    };
    let cases = builds.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(
            acc,
            "{}) u={}; s={} ;; ",
            b.platform,
            quote(b.url),
            b.sha256
        );
        acc
    });
    [
        AGENT_PATH_LINE.to_owned(),
        format!("{PROGRAM} --version >/dev/null 2>&1 && exit 0"),
        format!("d={dir}"),
        format!("if ! \"$d/{PROGRAM}\" --version >/dev/null 2>&1; then"),
        format!(
            "case \"$(uname -s) $(uname -m)\" in {cases}*) echo \"moor: herdr-core {HC_VERSION} has no build for $(uname -s) $(uname -m)\" >&2; exit 1 ;; esac"
        ),
        "for c in curl tar sha256sum mktemp; do command -v \"$c\" >/dev/null 2>&1 || { echo \"moor: the Box has no $c, so herdr-core cannot be installed\" >&2; exit 1; }; done"
            .to_owned(),
        "t=$(mktemp -d) || exit 1".to_owned(),
        "trap 'rm -rf \"$t\"' EXIT".to_owned(),
        format!(
            "curl -fsSL \"$u\" -o \"$t/hc.tar.gz\" || {}",
            fail(&format!("could not download herdr-core {HC_VERSION}"))
        ),
        "set -- $(sha256sum \"$t/hc.tar.gz\")".to_owned(),
        format!(
            "[ \"$1\" = \"$s\" ] || {}",
            fail(&format!(
                "the herdr-core {HC_VERSION} download failed its sha256 check"
            ))
        ),
        format!(
            "tar -xzf \"$t/hc.tar.gz\" -C \"$t\" {PROGRAM} && \"$t/{PROGRAM}\" --version >/dev/null 2>&1 || {}",
            fail(&format!(
                "the herdr-core {HC_VERSION} download does not run on this Box"
            ))
        ),
        format!("mkdir -p \"$d\" && mv -f \"$t/{PROGRAM}\" \"$d/{PROGRAM}\" || exit 1"),
        "fi".to_owned(),
        format!(
            "mkdir -p \"$HOME/.local/bin\" && ln -sf \"$d/{PROGRAM}\" \"$HOME/.local/bin/{PROGRAM}\" || exit 1"
        ),
        format!(
            "command -v hc >/dev/null 2>&1 || [ -e \"$HOME/.local/bin/hc\" ] || ln -s \"$d/{PROGRAM}\" \"$HOME/.local/bin/hc\""
        ),
        format!("{PROGRAM} --version >/dev/null 2>&1"),
    ]
    .join("\n")
}

/// The install a Box needs for the agent: herdr-core's first, in a subshell, as the agent's
/// script exits as soon as its command is there.
pub(crate) fn with_agent(agent: Option<AgentInstall>) -> AgentInstall {
    let own = hc_install();
    let Some(agent) = agent else {
        return own;
    };
    AgentInstall {
        script: format!("(\n{}\n) || exit 1\n{}", own.script, agent.script),
        present: format!("( {} ) && ( {} )", own.present, agent.present),
        dirs: [own.dirs, agent.dirs].concat(),
        check: format!("( {} ) && ( {} )", own.check, agent.check),
    }
}

#[cfg(test)]
#[path = "tests/holder.rs"]
mod tests;
