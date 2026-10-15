//! The brief the agent reads on start: what to do and how the Box works.

use std::fmt::Write;

/// What the brief says about one Task.
pub(super) struct BriefInput<'a> {
    /// The Task name.
    pub(super) task: &'a str,
    /// What the agent is asked to do.
    pub(super) goal: Option<&'a str>,
    /// The Issue text, when the Task came from one.
    pub(super) issue: Option<&'a str>,
    /// The Task branch the agent pushes to.
    pub(super) branch: &'a str,
    /// The exit code of a failed setup, if setup failed.
    pub(super) setup_exit: Option<i32>,
    /// The Task's own simulator device, on macOS.
    pub(super) sim_udid: Option<&'a str>,
}

/// The brief text for a Task.
pub(super) fn render(b: &BriefInput) -> String {
    let (task, branch) = (b.task, b.branch);
    let mut out = format!("# Task {task}\n\n");
    match (b.goal, b.issue) {
        (Some(g), Some(i)) => _ = write!(out, "Goal: {g}\n\nIssue: {i}\n\n"),
        (Some(g), None) => _ = write!(out, "Goal: {g}\n\n"),
        (None, Some(i)) => _ = write!(out, "Goal: resolve the Issue below.\n\nIssue: {i}\n\n"),
        (None, None) => {}
    }
    _ = write!(
        out,
        "You are running inside the Box, in the project working directory. Run builds, \
         tests and project tooling directly here.\n\n\
         Report the command's real exit code. Do not pipe its output through `tail` or `head`.\n\n\
         You run in a sandbox. You have no sudo. The network is open, through the preset \
         proxy (HTTPS_PROXY); do not unset or bypass it. GitHub access works for the Task \
         repo through gh and git as configured. Requests outside what the proxy rules allow go \
         out without credentials, so expect 401 or 404; the owner can add a rule. \
         You can write only in the working directory and your own tool state. If a tool the \
         project needs is missing, do not try to install it: say which tool is missing and that \
         it belongs in the repo's setup commands, then continue with what is available. \
         Do not guess paths.\n\n\
         HEAD is detached on the Box. Commit your work with git, following the repo's \
         commit-msg hook and conventions. Push with \
         `git push moor HEAD:refs/heads/{branch}` when the repo's push guard \
         allows it, not after each commit. If `git remote get-url moor` fails, commit \
         only, do not push, and say so. Push to the branch named in the Goal if it names one, else \
         to `{branch}`. Do not switch branches. Do not add Co-Authored-By trailers to \
         commits.\n\n\
         You may receive follow-up messages (steers) typed into this session.\n\n"
    );
    if let Some(n) = b.setup_exit {
        _ = write!(
            out,
            "Setup failed (exit {n}). Read the log at .moor/setup.log, fix the cause and \
             re-run the setup commands. If you cannot, present what you found as a Report at \
             `.moor/reports/<kind>.md`.\n\n"
        );
    }
    if let Some(udid) = b.sim_udid {
        _ = write!(
            out,
            "This Task has its own iOS simulator, already booted: UDID `{udid}`, also in \
             `$MOOR_SIM_UDID`. Use only this device: pass the UDID to `xcrun simctl` and \
             `xcodebuild -destination id={udid}`; never use `booted` and never touch other \
             simulators. Put proof under `.moor/proof/` (git ignores it): \
             `xcrun simctl io \"$MOOR_SIM_UDID\" screenshot \"$PWD/.moor/proof/<name>.png\"`, \
             and for video `xcrun simctl io \"$MOOR_SIM_UDID\" recordVideo --codec=h264 \
             \"$PWD/.moor/proof/<name>.mp4\"`, stopped with Ctrl-C (SIGINT). simctl needs an \
             absolute output path, so build it from `$PWD` as shown.\n\n"
        );
    }
    out.push_str(
        "Reports are how you show work to a human: a spec, a checkpoint, a result. \
         Kind is a free-form label (e.g. `spec`, `result`). To present a Report, write it as \
         a Markdown file at `.moor/reports/<kind>.md` in your working directory; do not \
         commit it. Writing again with the same kind adds a new version. Name that path in \
         your final message. A Report never means the Task is finished; keep working \
         until the goal is met, and do not stop after reporting.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(goal: Option<&'a str>, issue: Option<&'a str>) -> BriefInput<'a> {
        BriefInput {
            task: "t1",
            goal,
            issue,
            branch: "moor/t1",
            setup_exit: None,
            sim_udid: None,
        }
    }

    #[test]
    fn goal_and_issue() {
        let out = render(&input(Some("fix x"), Some("x is broken")));
        assert!(out.starts_with("# Task t1\n\n"));
        assert!(out.contains("Goal: fix x\n\nIssue: x is broken\n\n"));
    }

    #[test]
    fn goal_only() {
        let out = render(&input(Some("fix x"), None));
        assert!(out.contains("Goal: fix x\n\n"));
        assert!(!out.contains("Issue:"));
    }

    #[test]
    fn issue_only_states_the_goal() {
        let out = render(&input(None, Some("x is broken")));
        assert!(out.contains("Goal: resolve the Issue below.\n\nIssue: x is broken\n\n"));
    }

    #[test]
    fn states_the_box_contract() {
        let out = render(&input(Some("g"), None));
        assert!(out.contains("git push moor HEAD:refs/heads/moor/t1"));
        assert!(out.contains("HEAD is detached"));
        assert!(out.contains("Do not add Co-Authored-By trailers"));
        assert!(out.contains("follow-up messages (steers)"));
    }

    #[test]
    fn states_sandbox_limits_and_points_missing_tools_at_setup() {
        let out = render(&input(Some("g"), None));
        assert!(out.contains("no sudo"));
        assert!(!out.contains("including sudo"));
        assert!(out.contains("The network is open"));
        assert!(out.contains("expect 401 or 404"));
        assert!(out.contains("belongs in the repo's setup commands"));
        assert!(out.contains("Do not guess paths."));
    }

    #[test]
    fn setup_failure_is_named_only_when_it_failed() {
        assert!(!render(&input(Some("g"), None)).contains("Setup failed"));
        let mut b = input(Some("g"), None);
        b.setup_exit = Some(3);
        let out = render(&b);
        assert!(out.contains("Setup failed (exit 3)"));
        assert!(out.contains(".moor/setup.log"));
        assert!(out.contains(".moor/reports/<kind>.md"));
    }

    #[test]
    fn simulator_is_named_only_when_the_task_has_one() {
        let plain = render(&input(Some("g"), None));
        assert!(!plain.to_lowercase().contains("simulator"));
        assert!(!plain.contains(".moor/proof/"));
        let mut b = input(Some("g"), None);
        b.sim_udid = Some("ABCD-1234");
        let out = render(&b);
        assert!(out.contains("UDID `ABCD-1234`"));
        assert!(out.contains("id=ABCD-1234"));
        assert!(out.contains("\"$PWD/.moor/proof/<name>.png\""));
        assert!(out.contains("\"$PWD/.moor/proof/<name>.mp4\""));
        assert!(out.contains("simctl needs an absolute output path"));
        assert!(out.contains("never use `booted`"));
    }

    #[test]
    fn names_no_host_paths_moor_commands_or_secrets() {
        let mut b = input(Some("g"), Some("i"));
        b.setup_exit = Some(1);
        let out = render(&b);
        assert!(!out.contains("/home/"));
        assert!(!out.contains("moor run"));
        assert!(!out.contains("moor report"));
        assert!(!out.to_lowercase().contains("token"));
    }
}
