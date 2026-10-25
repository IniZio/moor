//! The brief the agent reads on start: what to do and how the Box works.

use std::fmt::Write;

use moor_core::shape::Shape;

/// What the brief says about one Box.
pub(super) struct BriefInput<'a> {
    /// The Box name.
    pub(super) name: &'a str,
    /// What the agent is asked to do.
    pub(super) goal: Option<&'a str>,
    /// The Issue text, when the Box came from one.
    pub(super) issue: Option<&'a str>,
    /// The Box branch the agent pushes to.
    pub(super) branch: &'a str,
    /// The exit code of a failed setup, if setup failed.
    pub(super) setup_exit: Option<i32>,
    /// The `brief:` of the repo's `.moor.yaml`.
    pub(super) repo_notes: Option<&'a str>,
    /// The CPUs and memory the Box delivers, when they are recorded.
    pub(super) shape: Option<Shape>,
}

/// The brief text for a Box.
pub(super) fn render(b: &BriefInput) -> String {
    let (name, branch) = (b.name, b.branch);
    let mut out = format!("# Box {name}\n\n");
    match (b.goal, b.issue) {
        (Some(g), Some(i)) => _ = write!(out, "Goal: {g}\n\nIssue: {i}\n\n"),
        (Some(g), None) => _ = write!(out, "Goal: {g}\n\n"),
        (None, Some(i)) => _ = write!(out, "Goal: resolve the Issue below.\n\nIssue: {i}\n\n"),
        (None, None) => {}
    }
    project_facts(b, &mut out);
    _ = write!(
        out,
        "You are running inside the machine, in the project working directory. Run builds, \
         tests and project tooling directly here.\n\n\
         Report the command's real exit code. Do not pipe its output through `tail` or `head`.\n\n\
         Do not use sudo. The network is open. When HTTPS_PROXY is set, it is a local proxy \
         that sends only the hosts that need moor's credentials through the moor Proxy and \
         every other host direct; do not unset or bypass it. GitHub access works for this Box's \
         repo through gh and git as configured. Requests outside what the proxy rules allow go \
         out without credentials, so expect 401 or 404; the owner can add a rule. \
         Write only in the working directory and your own tool state. If a tool the \
         project needs is missing, do not try to install it: say which tool is missing and that \
         it belongs in the repo's setup commands, then continue with what is available. \
         Do not guess paths. Before you tell the person a tool or program is missing, check for \
         it (for example `command -v <name>`, or the desktop's applications) and say what \
         you checked.\n\n\
         The checkout is on branch `{branch}`. Commit your work with git, following the repo's \
         commit-msg hook and conventions. Push with \
         `git push origin HEAD:refs/heads/{branch}` when the repo's push guard \
         allows it, not after each commit. If `git remote get-url origin` fails, commit \
         only, do not push, and say so. The checkout is a partial clone with full history: \
         git fetches file contents through the proxy when a command first needs them. \
         Push to the branch named in the Goal if it names one, else \
         to `{branch}`. Do not switch branches. Do not add Co-Authored-By trailers to \
         commits. When you opened a pull request, end your final message with its URL.\n\n\
         You may receive follow-up messages (steers) typed into this session.\n\n\
         Decide on your own when you reasonably can, and ask a question only when the \
         choice is the owner's.\n\n"
    );
    if let Some(n) = b.setup_exit {
        setup_failed(n, &mut out);
    }
    out.push_str(
        "Reports are how you show work to a human: a spec, a checkpoint, a result. \
         Kind is a free-form label (e.g. `spec`, `result`). To present a Report, write it as \
         a Markdown file at `.moor/reports/<kind>.md` in your working directory; do not \
         commit it. Writing again with the same kind adds a new version. Name that path in \
         your final message. A Report never means the Box is finished; keep working \
         until the goal is met, and do not stop after reporting.\n\n",
    );
    out.push_str(PUBLISH);
    out.push_str(ATTACH);
    out
}

/// How to show a human a file.
const PUBLISH: &str = "To show a human a file (a screenshot, a recording, a log), copy it into \
    `.moor/artifacts/` in your working directory, using a plain file name with its extension. \
    Do not commit it. Writing a file again with the same name adds a new version. It is \
    collected while you work, within about 30 seconds of the file being complete, and again \
    when you finish your turn. Name the files you published in your final message.\n\n";

/// What the agent does about a failed setup.
fn setup_failed(exit: i32, out: &mut String) {
    _ = write!(
        out,
        "Setup failed (exit {exit}). Read the log at .moor/setup.log, fix the cause and \
         re-run the setup commands. If you cannot, present what you found as a Report at \
         `.moor/reports/<kind>.md`.\n\n"
    );
}

/// The repo author's notes and what the Box delivers, when they are known.
fn project_facts(b: &BriefInput, out: &mut String) {
    if let Some(notes) = b.repo_notes {
        _ = write!(
            out,
            "## This project's Box\n\nFrom the repo's `.moor.yaml`:\n\n{notes}\n\n"
        );
    }
    if let Some(Shape { cpus, mem_mib }) = b.shape {
        let tenths = mem_mib.saturating_mul(10) / 1024;
        let plural = if cpus == 1 { "" } else { "s" };
        _ = write!(
            out,
            "This Box has {cpus} vCPU{plural} and {}.{} GiB of memory.\n\n",
            tenths / 10,
            tenths % 10
        );
    }
}

/// How to show artifacts on a pull request; the limits are gh's own.
const ATTACH: &str = "To show a published file on a pull request, attach it with gh: \
    `gh pr create --attach .moor/artifacts/<name>` or \
    `gh pr edit <number> --attach .moor/artifacts/<name>`. Write \
    `![<name>](.moor/artifacts/<name>)` in the body where it should appear. gh replaces \
    it with the uploaded URL. gh attaches png, jpg, jpeg, gif, webp, svg, mp4, mov and \
    webm files: images up to 10 MB, videos up to 100 MB on paid plans, at most 50 \
    files. An upload cannot be deleted.\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(goal: Option<&'a str>, issue: Option<&'a str>) -> BriefInput<'a> {
        BriefInput {
            name: "t1",
            goal,
            issue,
            branch: "moor/t1",
            setup_exit: None,
            repo_notes: None,
            shape: None,
        }
    }

    #[test]
    fn goal_and_issue() {
        let out = render(&input(Some("fix x"), Some("x is broken")));
        assert!(out.starts_with("# Box t1\n\n"));
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
        assert!(out.contains("git push origin HEAD:refs/heads/moor/t1"));
        assert!(out.contains("on branch `moor/t1`"));
        assert!(out.contains("partial clone"));
        assert!(!out.contains("moor HEAD"), "{out}");
        assert!(out.contains("Do not add Co-Authored-By trailers"));
        assert!(out.contains("follow-up messages (steers)"));
    }

    #[test]
    fn asks_for_the_pull_request_url_at_the_end_of_the_final_message() {
        let out = render(&input(Some("g"), None));
        assert!(
            out.contains("When you opened a pull request, end your final message with its URL."),
            "{out}"
        );
    }

    #[test]
    fn asks_only_when_the_choice_is_the_owners() {
        let out = render(&input(Some("g"), None));
        assert!(out.contains(
            "Decide on your own when you reasonably can, and ask a question only when the choice is the owner's.\n\n"
        ));
    }

    #[test]
    fn names_no_moor_remote_and_no_shallow_history() {
        let out = render(&input(Some("g"), None));
        assert!(!out.contains("shallow"), "{out}");
        assert!(!out.contains(" moor`"), "{out}");
    }

    #[test]
    fn says_how_to_attach_artifacts_to_a_pull_request_right_after_publishing() {
        let attach = "To show a published file on a pull request, attach it with gh: `gh pr create --attach .moor/artifacts/<name>` or `gh pr edit <number> --attach .moor/artifacts/<name>`. Write `![<name>](.moor/artifacts/<name>)` in the body where it should appear. gh replaces it with the uploaded URL. gh attaches png, jpg, jpeg, gif, webp, svg, mp4, mov and webm files: images up to 10 MB, videos up to 100 MB on paid plans, at most 50 files. An upload cannot be deleted.\n";
        let out = render(&input(Some("g"), None));
        assert!(
            out.contains(&format!("in your final message.\n\n{attach}")),
            "{out}"
        );
    }

    #[test]
    fn says_how_to_publish_artifacts() {
        let out = render(&input(Some("g"), None));
        assert!(out.contains(".moor/artifacts/"));
        assert!(out.contains("Do not commit it"));
        assert!(out.contains("adds a new version"));
        assert!(out.contains("when you finish your turn"));
        assert!(out.contains("Name the files you published in your final message"));
    }

    #[test]
    fn states_limits_and_points_missing_tools_at_setup() {
        let out = render(&input(Some("g"), None));
        assert!(out.contains("Do not use sudo."));
        assert!(!out.contains("sandbox"));
        assert!(out.contains("The network is open."));
        assert!(out.contains("When HTTPS_PROXY is set"));
        assert!(out.contains("Write only in the working directory"));
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
    fn repo_notes_follow_the_goal_under_their_own_heading_verbatim() {
        let mut b = input(Some("fix x"), None);
        b.repo_notes = Some("Start the emulator:\n  make emu\n");
        let out = render(&b);
        let goal = out.find("Goal: fix x").unwrap();
        let heading = "## This project's Box\n\nFrom the repo's `.moor.yaml`:\n\nStart the emulator:\n  make emu\n\n";
        assert!(out.contains(heading), "{out}");
        let head = out.find(heading).unwrap();
        assert!(goal < head, "{out}");
        assert!(head < out.find("You are running inside the machine").unwrap());
    }

    #[test]
    fn no_repo_notes_no_heading() {
        let out = render(&input(Some("g"), None));
        assert!(!out.contains("## This project's Box"), "{out}");
    }

    #[test]
    fn states_cpus_and_memory_when_known() {
        let mut b = input(Some("g"), None);
        b.shape = Some(Shape {
            cpus: 4,
            mem_mib: 7900,
        });
        let out = render(&b);
        assert!(
            out.contains("This Box has 4 vCPUs and 7.7 GiB of memory."),
            "{out}"
        );
        b.shape = Some(Shape {
            cpus: 1,
            mem_mib: 2048,
        });
        assert!(render(&b).contains("1 vCPU and 2.0 GiB of memory."));
        assert!(!render(&input(Some("g"), None)).contains("vCPU"));
    }

    #[test]
    fn says_to_check_before_calling_a_tool_missing() {
        let out = render(&input(Some("g"), None));
        assert!(
            out.contains(
                "Before you tell the person a tool or program is missing, check for it \
                 (for example `command -v <name>`, or the desktop's applications) and say what \
                 you checked."
            ),
            "{out}"
        );
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
