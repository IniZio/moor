//! What holds the agent's terminal on the Box, so a dropped link leaves the agent running and a
//! new pane can attach to it: one table of Box commands per holder program.

use std::fmt::Write as _;

use moor_core::boxscript::quote;
use moor_core::config::AgentHolder;
use moor_core::seam::agent_adapter::AgentInstall;

use super::AGENT_PATH_LINE;

/// The name of the agent's terminal; a Box runs one agent at a time.
pub(in crate::adapters::box_agent) const SESSION: &str = "moor-agent";

/// The Box commands of one holder. Each runs under `sh` after [`AGENT_PATH_LINE`].
pub(crate) struct Holder {
    /// The Box program; the start refuses to run the agent without it.
    pub(super) program: &'static str,
    /// Replaces any agent terminal with a detached one running the shell line `inner`, after
    /// removing the shell-quoted `exit_file`; exits the shell when the terminal did not start.
    pub(super) open: fn(exit_file: &str, inner: &str) -> String,
    /// Exits 0 only while the agent's terminal is live.
    pub(super) alive: fn() -> String,
    /// Puts the calling terminal on the agent's terminal until the terminal ends.
    pub(super) attach: fn() -> String,
    /// What a relayed client execs: it attaches, and sets how the size follows the clients.
    pub(super) relay: fn() -> String,
    /// Ends the agent's terminal, which hangs the agent up.
    pub(super) kill: fn() -> String,
    /// How moor installs the holder on a Box that lacks it; `None` when the image brings it.
    install: Option<fn() -> AgentInstall>,
}

/// The holder `kind` names.
pub(crate) fn holder(kind: AgentHolder) -> &'static Holder {
    match kind {
        AgentHolder::Hc => &HC,
        AgentHolder::Tmux => &TMUX,
    }
}

/// herdr-core, called by its own name: `hc` is also herbstclient's.
pub(crate) static HC: Holder = Holder {
    program: HC_BIN,
    open: hc_open,
    alive: || format!("{HC_BIN} get {SESSION} --json 2>/dev/null | grep -q '\"state\":\"live\"'"),
    attach: || format!("{HC_BIN} attach {SESSION}"),
    relay: || format!("exec {HC_BIN} attach {SESSION}"),
    kill: || format!("{HC_BIN} kill {SESSION} >/dev/null"),
    install: Some(hc_install),
};

/// The herdr-core program.
const HC_BIN: &str = "herdr-core";

/// A terminal with no parent, so no other hc terminal's end takes it down, removed once it
/// exits, so a finished one never answers for the name.
fn hc_open(exit_file: &str, inner: &str) -> String {
    format!(
        "{HC_BIN} kill {SESSION} >/dev/null 2>&1; rm -f {exit_file}; \
         {HC_BIN} run --bg --no-parent --rm --name {SESSION} sh -c {} >/dev/null || exit 1;",
        quote(inner)
    )
}

/// tmux, which the Box image must bring.
pub(crate) static TMUX: Holder = Holder {
    program: "tmux",
    open: tmux_open,
    alive: || format!("tmux has-session -t {SESSION} 2>/dev/null"),
    attach: || format!("tmux attach-session -t {SESSION}"),
    relay: tmux_relay,
    kill: || format!("tmux kill-session -t {SESSION}"),
    install: None,
};

/// The agent window's own size, as tmux size flags.
const WINDOW: &str = "-x 200 -y 50";

/// The tmux hook run when a client attaches: it writes the status the agent last printed, kept
/// in the user option `@moor_status_file`, straight to that client's terminal, so a client that
/// joins after a change still sees the current state. Unlike the agent's own print it is not
/// wrapped for passthrough, as it does not travel through tmux.
const REPLAY_ON_ATTACH: &str = r##"run-shell -b 'f="#{@moor_status_file}"; [ -s "$f" ] && printf "\033]7501;%s\033\134" "$(cat "$f")" > "#{client_tty}" 2>/dev/null; :'"##;

/// A session with no status line or prefix key that passes the agent's status through.
fn tmux_open(exit_file: &str, inner: &str) -> String {
    format!(
        "tmux kill-session -t {SESSION} 2>/dev/null; rm -f {exit_file}; \
         tmux new-session -d -s {SESSION} {WINDOW} {} || exit 1; \
         tmux set-option -t {SESSION} status off >/dev/null; \
         tmux set-option -t {SESSION} prefix None >/dev/null; \
         tmux set-option -t {SESSION} allow-passthrough on >/dev/null 2>&1; \
         tmux set-option -s escape-time 0 >/dev/null; \
         tmux set-option -t {SESSION} @moor_status_file \"$(dirname {exit_file})/status-osc\" >/dev/null; \
         tmux set-hook -t {SESSION} client-attached {} >/dev/null;",
        quote(inner),
        quote(REPLAY_ON_ATTACH),
    )
}

/// While a client is attached the agent's window takes the size of the client that used it
/// last; once the last client leaves tmux puts the window back to its own size.
fn tmux_relay() -> String {
    format!(
        "tmux set-hook -t {SESSION} client-detached \
         'if -F \"#{{==:#{{session_attached}},0}}\" \"resize-window -t {SESSION} {WINDOW}\"' \
         >/dev/null 2>&1; \
         tmux set-option -w -t {SESSION} window-size latest >/dev/null 2>&1; \
         exec tmux attach-session -t {SESSION}"
    )
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
        present: format!("{AGENT_PATH_LINE}\n{HC_BIN} --version >/dev/null 2>&1"),
        dirs: vec![HC_DIR.to_owned()],
        check: format!(
            "for f in \"$HOME\"/{}/*/{HC_BIN}; do [ ! -e \"$f\" ] || \"$f\" --version >/dev/null 2>&1 || exit 1; done",
            HC_DIR.trim_start_matches("~/")
        ),
    }
}

/// The script that installs herdr-core from `builds` when the Box has none that runs. The
/// download is checked against its pinned sha256 before it is unpacked; a Box that restored the
/// install directory is linked to it and downloads nothing. `hc` is linked too unless the Box
/// has one already.
pub(super) fn hc_install_script(builds: &[Build]) -> String {
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
        format!("{HC_BIN} --version >/dev/null 2>&1 && exit 0"),
        format!("d={dir}"),
        format!("if ! \"$d/{HC_BIN}\" --version >/dev/null 2>&1; then"),
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
            "tar -xzf \"$t/hc.tar.gz\" -C \"$t\" {HC_BIN} && \"$t/{HC_BIN}\" --version >/dev/null 2>&1 || {}",
            fail(&format!(
                "the herdr-core {HC_VERSION} download does not run on this Box"
            ))
        ),
        format!("mkdir -p \"$d\" && mv -f \"$t/{HC_BIN}\" \"$d/{HC_BIN}\" || exit 1"),
        "fi".to_owned(),
        format!(
            "mkdir -p \"$HOME/.local/bin\" && ln -sf \"$d/{HC_BIN}\" \"$HOME/.local/bin/{HC_BIN}\" || exit 1"
        ),
        format!(
            "command -v hc >/dev/null 2>&1 || [ -e \"$HOME/.local/bin/hc\" ] || ln -s \"$d/{HC_BIN}\" \"$HOME/.local/bin/hc\""
        ),
        format!("{HC_BIN} --version >/dev/null 2>&1"),
    ]
    .join("\n")
}

/// The install a Box needs for the agent `agent` held by `h`: the holder's first, in a subshell,
/// as the agent's script exits as soon as its command is there.
pub(crate) fn with_agent(h: &Holder, agent: Option<AgentInstall>) -> Option<AgentInstall> {
    let Some(own) = h.install.map(|f| f()) else {
        return agent;
    };
    let Some(agent) = agent else {
        return Some(own);
    };
    Some(AgentInstall {
        script: format!("(\n{}\n) || exit 1\n{}", own.script, agent.script),
        present: format!("( {} ) && ( {} )", own.present, agent.present),
        dirs: [own.dirs, agent.dirs].concat(),
        check: format!("( {} ) && ( {} )", own.check, agent.check),
    })
}

#[cfg(test)]
#[path = "tests/holder.rs"]
mod tests;
