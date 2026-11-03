//! The script that starts the agent in the Box.

use moor_core::boxscript::profile::PLUGIN_SECS_FILE;
use moor_core::boxscript::{command_line, quote};
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_adapter::{BoxSetup, LaunchCtx};
use moor_core::seam::proc::Tool;

use super::super::agent_env;
use super::super::probe::HOOK_FILE;
use super::super::profile_writer::FAILURES_FILE;
use super::{BRIEF_FILE, SCRIPT_FILE, STATUS_OSC_FILE};

/// Where the agent script lands on the Box.
pub(super) fn script_path(cred: &BoxPath) -> Result<BoxPath, NameError> {
    BoxPath::under(cred.as_str(), SCRIPT_FILE)
}

/// [`script_stripping`] with no names to strip.
#[cfg(test)]
pub(super) fn script(
    paths: (&BoxPath, &BoxPath, &str),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
    (proxy, env): (&[String], &[String]),
) -> Result<String, NameError> {
    script_stripping(paths, ctx, hooks, preseed, (argv, &[]), (proxy, env, &[]))
}

/// The line that puts the agent's install directories on the Box's `PATH`, in the launch script
/// and in the check that the agent command exists.
pub(in crate::adapters::box_agent) const AGENT_PATH_LINE: &str =
    "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"";

/// The script the Box runs under `sh`; the credential directory goes once the brief is
/// installed, and at exit or on a hangup, interrupt or terminate before then. The `strip_env`
/// names and crabbox's own names are unset before anything else runs, and the `env` exports run
/// just before the profile `writers`, which run last before the agent, with the agent on `PATH`
/// and the Proxy reachable. The starting marker it writes to the hook file names the launch by
/// `nonce`.
pub(super) fn script_stripping(
    (cred, tmp, nonce): (&BoxPath, &BoxPath, &str),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    (argv, writers): (&[String], &[BoxSetup]),
    (proxy, env, strip_env): (&[String], &[String], &[String]),
) -> Result<String, NameError> {
    let workdir = ctx.workdir.as_str();
    let moor = quote(BoxPath::under(workdir, ".moor")?.as_str());
    let brief = quote(BoxPath::under(workdir, BRIEF_FILE)?.as_str());
    let state = quote(BoxPath::under(workdir, HOOK_FILE)?.as_str());
    let staged = quote(BoxPath::under(cred.as_str(), "brief.md")?.as_str());
    let agent = std::iter::once(Tool::Agent.program().to_owned())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    let remove = format!("rm -rf {}", quote(cred.as_str()));
    let remove_all = format!("{remove} {}", quote(tmp.as_str()));
    let tmp = quote(tmp.as_str());
    let mut lines = vec![
        format!("trap {} EXIT", quote(&remove_all)),
        "trap 'exit 129' HUP".to_owned(),
        "trap 'exit 130' INT".to_owned(),
        "trap 'exit 143' TERM".to_owned(),
        format!("mkdir -p -m 700 {tmp} && mkdir -p {moor} && install -m 600 {staged} {brief} || exit 1"),
        format!(
            "printf '{{\"state\":\"starting\",\"event\":\"Launch\",\"launch\":\"%s\",\"ts\":%s000}}\\n' {} \"$(date +%s)\" > {state} || exit 1",
            quote(nonce)
        ),
        format!("{} || exit 1", hooks.step),
        format!(
            "MOOR_AGENT_TTY=$(tty 2>/dev/null); case $MOOR_AGENT_TTY in /dev/*) ;; *) MOOR_AGENT_TTY= ;; esac; export MOOR_AGENT_TTY; rm -f {}",
            quote(BoxPath::under(workdir, STATUS_OSC_FILE)?.as_str())
        ),
        preseed.step.clone(),
        AGENT_PATH_LINE.to_owned(),
        format!("export TMPDIR={tmp} CLAUDE_CODE_TMPDIR={tmp}"),
    ];
    // The platform's secrets and crabbox's names go before anything else runs.
    let mut at = 4;
    if !strip_env.is_empty() {
        let names = strip_env.iter().map(|n| quote(n)).collect::<Vec<_>>();
        lines.insert(at, format!("unset {}", names.join(" ")));
        at += 1;
    }
    for line in agent_env::clear_lines() {
        lines.insert(at, line);
        at += 1;
    }
    lines.insert(at + 1, remove);
    lines.extend(proxy.iter().cloned());
    lines.extend(env.iter().cloned());
    if !writers.is_empty() {
        // Each writer rewrites the failures file, so each one's lines are kept as it ends.
        let failures = BoxPath::under(workdir, FAILURES_FILE)?;
        let all = quote(&format!("{}.all", failures.as_str()));
        let failures = quote(failures.as_str());
        lines.push(format!(": > {all}"));
        let secs = BoxPath::under(workdir, PLUGIN_SECS_FILE)?;
        lines.push(format!(": > {}", quote(secs.as_str())));
        // Each writer's time is one line, which the watcher sums into the `plugins` step.
        let secs = quote(secs.as_str());
        for w in writers {
            lines.push("t0=$(date +%s)".to_owned());
            lines.push(w.step.clone());
            lines.push(format!("printf '%s\\n' $(( $(date +%s) - t0 )) >> {secs}"));
            lines.push(format!("cat {failures} >> {all}"));
        }
        lines.push(format!("mv {all} {failures}"));
    }
    lines.push(format!("cd {} && {}", quote(workdir), command_line(&agent)));
    if let Some(hook) = hooks.files.first() {
        // The agent has no end-of-run hook that fires on every exit, so the script reports it.
        lines.push("moor_rc=$?".to_owned());
        lines.push(format!(
            "sh {} done SessionEnd </dev/null",
            quote(hook.path.as_str())
        ));
        lines.push("exit $moor_rc".to_owned());
    }
    Ok(lines.join("\n"))
}
