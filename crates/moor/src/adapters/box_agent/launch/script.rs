//! The script that starts the agent in the Box.

use moor_core::boxscript::{command_line, quote};
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_adapter::{BoxSetup, LaunchCtx};
use moor_core::seam::proc::Tool;

use super::super::agent_env;
use super::super::profile_writer::FAILURES_FILE;
use super::{AGENT_PATH_LINE, BRIEF_FILE, SCRIPT_FILE};

/// The line that removes the temp dirs of the Box's earlier launches, all but `tmp`: the agent
/// is exec'd, so no exit trap removes its own.
fn remove_earlier(tmp: &BoxPath) -> String {
    tmp.as_str()
        .rsplit_once('-')
        .map(|(stem, _)| {
            format!(
                "for d in {}-*; do [ \"$d\" = {} ] || rm -rf \"$d\"; done",
                quote(stem),
                quote(tmp.as_str())
            )
        })
        .unwrap_or_default()
}

/// Where the agent script lands on the Box.
pub(super) fn script_path(cred: &BoxPath) -> Result<BoxPath, NameError> {
    BoxPath::under(cred.as_str(), SCRIPT_FILE)
}

/// [`script_stripping`] with no names to strip.
#[cfg(test)]
pub(super) fn script(
    paths: (&BoxPath, &BoxPath),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
    (proxy, env): (&[String], &[String]),
) -> Result<String, NameError> {
    script_stripping(paths, ctx, hooks, preseed, (argv, &[]), (proxy, env, &[]))
}

/// The script the Box runs under `sh`; the credential directory goes once the brief is
/// installed, and at exit or on a hangup, interrupt or terminate before then. The `strip_env`
/// names and crabbox's own names are unset before anything else runs, and the `env` exports run
/// just before the profile `writers`, which run last before the agent, with the agent on `PATH`
/// and the Proxy reachable.
pub(super) fn script_stripping(
    (cred, tmp): (&BoxPath, &BoxPath),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    (argv, writers): (&[String], &[BoxSetup]),
    (proxy, env, strip_env): (&[String], &[String], &[String]),
) -> Result<String, NameError> {
    let workdir = ctx.workdir.as_str();
    let moor = quote(BoxPath::under(workdir, ".moor")?.as_str());
    let brief = quote(BoxPath::under(workdir, BRIEF_FILE)?.as_str());
    let staged = quote(BoxPath::under(cred.as_str(), "brief.md")?.as_str());
    let agent = std::iter::once(Tool::Agent.program().to_owned())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    let remove = format!("rm -rf {}", quote(cred.as_str()));
    let remove_all = format!("{remove} {}", quote(tmp.as_str()));
    let earlier = remove_earlier(tmp);
    let tmp = quote(tmp.as_str());
    let mut lines = vec![
        format!("trap {} EXIT", quote(&remove_all)),
        "trap 'exit 129' HUP".to_owned(),
        "trap 'exit 130' INT".to_owned(),
        "trap 'exit 143' TERM".to_owned(),
        format!(
            "mkdir -p -m 700 {tmp} && mkdir -p {moor} && install -m 600 {staged} {brief} || exit 1"
        ),
        earlier,
        format!("{} || exit 1", hooks.step),
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
        for w in writers {
            lines.push(w.step.clone());
            lines.push(format!("cat {failures} >> {all}"));
        }
        lines.push(format!("mv {all} {failures}"));
    }
    lines.push(format!(
        "cd {} && exec {}",
        quote(workdir),
        command_line(&agent)
    ));
    Ok(lines.join("\n"))
}
