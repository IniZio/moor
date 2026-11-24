//! The brief the agent keeps as standing instructions: how the Box works.

use std::fmt::Write;

use moor_core::seam::engine::HeldRepo;
use moor_core::shape::Shape;

mod repos;

/// What the brief says about one Box.
pub(super) struct BriefInput<'a> {
    /// The Box name.
    pub(super) name: &'a str,
    /// The Issue text, when the Box came from one.
    pub(super) issue: Option<&'a str>,
    /// The Box branch the agent pushes to.
    pub(super) branch: &'a str,
    /// The exit code of a failed setup, if setup failed.
    pub(super) setup_exit: Option<i32>,
    /// The `brief:` of the repo's config.
    pub(super) repo_notes: Option<&'a str>,
    /// The CPUs and memory the Box delivers, when they are recorded.
    pub(super) shape: Option<Shape>,
    /// Every repo the Box holds and its work tree, the agent's own first.
    pub(super) repos: &'a [HeldRepo],
    /// Whether the Box is an agent place, which holds no checkout and takes its repos from each
    /// task's first message.
    pub(super) place: bool,
}

/// The brief text for a Box.
pub(super) fn render(b: &BriefInput) -> String {
    let name = b.name;
    let mut out = format!("# Box {name}\n\n");
    if let Some(i) = b.issue {
        _ = write!(out, "Issue: {i}\n\n");
    }
    project_facts(b, &mut out);
    if b.place {
        out.push_str(PLACE);
    } else {
        work(b, &mut out);
    }
    out.push_str(
        "Reports are how you show work to a human: a spec, a checkpoint, a result. \
         Kind is a free-form label (e.g. `spec`, `result`). To present a Report, write it as \
         a Markdown file at `.moor/reports/<kind>.md` in your working directory; do not \
         commit it. Writing again with the same kind adds a new version. Name that path in \
         your final message. A Report never means the Box is finished; keep working \
         until the work is done, and do not stop after reporting.\n\n",
    );
    out.push_str(PUBLISH);
    out.push_str(ATTACH);
    out.push_str(GH_INSTALL);
    out
}

/// How an agent place works: no checkout, and the repos of each task named in its first message.
const PLACE: &str = "You are running inside the machine, in an agent place: it holds no \
    checkout and no branch, and waits for a task. Each task's first message starts with a \
    `Repos:` line naming the GitHub repos it is about, then a blank line, then the person's \
    message. Read those repos through GitHub: `gh api`, or \
    `git clone --filter=blob:none https://github.com/OWNER/NAME /tmp/NAME`. Requests for \
    the repos the task names carry the owner's credential through moor's Proxy; any other \
    repo answers 401 or 404. When HTTPS_PROXY is set, do not unset or bypass it. Write only \
    in /tmp, your own tool state and the tools you install. Answer in your final message.\n\n\
    You may receive follow-up messages (steers) typed into this session.\n\n\
    Decide on your own when you reasonably can, and ask a question only when the choice is \
    the owner's.\n\n";

/// How a work Box works: its checkout, its branch and its repos.
fn work(b: &BriefInput, out: &mut String) {
    let branch = b.branch;
    _ = write!(
        out,
        "You are running inside the machine, in the project working directory. Run builds, \
         tests and project tooling directly here.\n\n\
         Report the command's real exit code. Do not pipe its output through `tail` or `head`.\n\n\
         `sudo -n` may work here: test it before you say you lack access, and use it when the \
         project's tools need it. Anything later Boxes also need belongs in the repo's setup \
         commands. The network is open. When HTTPS_PROXY is set, it is a local proxy \
         that sends only the hosts that need moor's credentials through the moor Proxy and \
         every other host direct; do not unset or bypass it. GitHub access works for this Box's \
         repos through git, and through gh when gh is on the PATH. Requests outside what the proxy rules allow go \
         out without credentials, so expect 401 or 404; the owner can add a rule. \
         Write only in the working directory, your own tool state and the tools you install. \
         If a tool the work needs is missing, install it, then say which tool it was. \
         Do not guess paths. Before you tell the person a tool or program is missing, check for \
         it (for example `command -v <name>`, or the desktop's applications) and say what \
         you checked.\n\n\
         The checkout is on branch `{branch}`. Commit your work with git, following the repo's \
         commit-msg hook and conventions. Push with \
         `git push origin HEAD:refs/heads/{branch}` when the repo's push guard \
         allows it, not after each commit. If `git remote get-url origin` fails, commit \
         only, do not push, and say so. The checkout is a partial clone with full history: \
         git fetches file contents through the proxy when a command first needs them. \
         Push to the branch the first prompt names, if it names one, \
         else to `{branch}`. Do not switch branches. Do not add Co-Authored-By trailers to \
         commits. When you opened a pull request, end your final message with its URL.\n\n\
         You may receive follow-up messages (steers) typed into this session.\n\n\
         Decide on your own when you reasonably can, and ask a question only when the \
         choice is the owner's.\n\n"
    );
    repos::section(b.repos, branch, out);
    if let Some(n) = b.setup_exit {
        setup_failed(n, out);
    }
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
            "## This project's Box\n\nFrom the repo's config:\n\n{notes}\n\n"
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

/// How to open a pull request and show artifacts on it; the limits are gh's own.
const ATTACH: &str = "When gh is on the PATH, attach a published file to a pull request with \
    `gh pr create --attach .moor/artifacts/<name>` or \
    `gh pr edit <number> --attach .moor/artifacts/<name>`. Write \
    `![<name>](.moor/artifacts/<name>)` in the body where it should appear. gh replaces \
    it with the uploaded URL. gh attaches png, jpg, jpeg, gif, webp, svg, mp4, mov and \
    webm files: images up to 10 MB, videos up to 100 MB on paid plans, at most 50 \
    files. An upload cannot be deleted.\n\n";

/// How to get gh when the Box image lacks it: a pinned release, checked against its sha256.
const GH_INSTALL: &str = r#"When `command -v gh` finds nothing, install gh into `~/.local/bin` with this, then run gh as usual:

```sh
v=2.105.0; f=gh_${v}_linux_$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').tar.gz
curl -fsSL --retry 3 -o /tmp/$f https://github.com/cli/cli/releases/download/v$v/$f
printf '%s\n' \
  '79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f  gh_2.105.0_linux_amd64.tar.gz' \
  'b1278c75107423d818f1937fd92cafc9e5e9b606631568f68d7fb816f5885a02  gh_2.105.0_linux_arm64.tar.gz' \
  | grep " $f" | (cd /tmp && sha256sum -c --quiet) \
  && tar -xzf /tmp/$f -C /tmp && install -D /tmp/${f%.tar.gz}/bin/gh ~/.local/bin/gh
```

gh reaches GitHub as the Box owner through moor's Proxy. gh's environment holds only a placeholder, and the Proxy puts the owner's credential on each request its rules allow. Do not run `gh auth login` or change gh's credentials or config. A 401 or 404 means the Proxy withheld the credential, as it does for repos outside this Box. Posting a comment on an issue or pull request (`gh pr comment`, `gh issue comment`, or `POST /repos/{owner}/{repo}/issues/<number>/comments`) is withheld unless this Box's config asks for it, so a 401 there is expected: do not retry or look for a way around it, and put what you would have said in your report.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn input(issue: Option<&str>) -> BriefInput<'_> {
        BriefInput {
            name: "t1",
            issue,
            branch: "moor/t1",
            setup_exit: None,
            repo_notes: None,
            shape: None,
            repos: &[],
            place: false,
        }
    }

    #[test]
    fn a_place_brief_names_no_checkout_or_branch() {
        let out = render(&BriefInput {
            place: true,
            ..input(None)
        });
        assert!(out.contains("`Repos:` line"), "{out}");
        assert!(
            !out.contains("moor/t1") && !out.contains("The checkout is on branch"),
            "{out}"
        );
    }

    #[test]
    fn the_clone_a_place_brief_names_passes_the_places_git_rule() {
        use moor_core::config::proxy::{resolve_rules, ProxyConfig};
        use nono_proxy::config::{CompiledEndpointRules, EndpointRule};

        let out = render(&BriefInput {
            place: true,
            ..input(None)
        });
        let url = out
            .split_whitespace()
            .find(|w| w.starts_with("https://github.com/OWNER/NAME"))
            .expect("the place brief names a clone URL")
            .trim_end_matches('`')
            .replace("OWNER/NAME", "o/n");
        let refs = format!("{}/info/refs", url.trim_start_matches("https://github.com"));
        let rules = resolve_rules(
            &ProxyConfig::default(),
            &[moor_core::seam::forge::ForgeRepo::new("o", "n")],
            false,
        )
        .unwrap();
        let git = rules.iter().find(|r| r.name == "github-git").unwrap();
        let allow: Vec<EndpointRule> = git
            .endpoint_rules
            .iter()
            .map(|r| EndpointRule {
                method: r.method.clone(),
                path: r.path.clone(),
            })
            .collect();
        let allow = CompiledEndpointRules::compile(&allow).unwrap();
        assert!(allow.is_allowed("GET", &refs), "{refs}");
    }

    #[test]
    fn names_no_prompt_or_goal_line() {
        let out = render(&input(None));
        assert!(
            out.starts_with("# Box t1\n\nYou are running inside the machine"),
            "{out}"
        );
        assert!(!out.contains("Goal:") && !out.contains("Prompt:"), "{out}");
    }

    #[test]
    fn names_the_issue() {
        let out = render(&input(Some("x is broken")));
        assert!(out.contains("# Box t1\n\nIssue: x is broken\n\n"));
    }

    #[test]
    fn states_the_box_contract() {
        let out = render(&input(None));
        assert!(out.contains("git push origin HEAD:refs/heads/moor/t1"));
        assert!(out.contains("on branch `moor/t1`"));
        assert!(out.contains("partial clone"));
        assert!(!out.contains("moor HEAD"), "{out}");
        assert!(out.contains("Do not add Co-Authored-By trailers"));
        assert!(out.contains("follow-up messages (steers)"));
    }

    #[test]
    fn asks_for_the_pull_request_url_at_the_end_of_the_final_message() {
        let out = render(&input(None));
        assert!(
            out.contains("When you opened a pull request, end your final message with its URL."),
            "{out}"
        );
    }

    #[test]
    fn asks_only_when_the_choice_is_the_owners() {
        let out = render(&input(None));
        assert!(out.contains(
            "Decide on your own when you reasonably can, and ask a question only when the choice is the owner's.\n\n"
        ));
    }

    #[test]
    fn names_no_moor_remote_and_no_shallow_history() {
        let out = render(&input(None));
        assert!(!out.contains("shallow"), "{out}");
        assert!(!out.contains(" moor`"), "{out}");
    }

    #[test]
    fn says_how_to_attach_artifacts_to_a_pull_request_right_after_publishing() {
        let attach = "When gh is on the PATH, attach a published file to a pull request with `gh pr create --attach .moor/artifacts/<name>` or `gh pr edit <number> --attach .moor/artifacts/<name>`. Write `![<name>](.moor/artifacts/<name>)` in the body where it should appear. gh replaces it with the uploaded URL. gh attaches png, jpg, jpeg, gif, webp, svg, mp4, mov and webm files: images up to 10 MB, videos up to 100 MB on paid plans, at most 50 files. An upload cannot be deleted.\n\n";
        let out = render(&input(None));
        assert!(
            out.contains(&format!("in your final message.\n\n{attach}")),
            "{out}"
        );
    }

    #[test]
    fn says_how_to_install_a_pinned_gh_that_signs_in_through_the_proxy() {
        let out = render(&input(None));
        let recipe = out
            .split("```sh\n")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .expect("an sh block");
        let check = std::process::Command::new("bash")
            .args(["-n", "-c", recipe])
            .status()
            .expect("bash");
        assert!(check.success(), "{recipe}");
        for arch in ["amd64", "arm64"] {
            assert!(
                recipe.contains(&format!("  gh_2.105.0_linux_{arch}.tar.gz'")),
                "{recipe}"
            );
        }
        assert!(recipe.contains("sha256sum -c"), "{recipe}");
        assert!(out.contains("Do not run `gh auth login`"), "{out}");
    }

    #[test]
    fn says_how_to_publish_artifacts() {
        let out = render(&input(None));
        assert!(out.contains(".moor/artifacts/"));
        assert!(out.contains("Do not commit it"));
        assert!(out.contains("adds a new version"));
        assert!(out.contains("when you finish your turn"));
        assert!(out.contains("Name the files you published in your final message"));
    }

    #[test]
    fn states_limits_and_points_missing_tools_at_setup() {
        let out = render(&input(None));
        assert!(out.contains(
            "`sudo -n` may work here: test it before you say you lack access, and use it when \
             the project's tools need it. Anything later Boxes also need belongs in the repo's \
             setup commands."
        ));
        assert!(!out.contains("Do not use sudo"));
        assert!(!out.contains("sandbox"));
        assert!(out.contains("The network is open."));
        assert!(out.contains("When HTTPS_PROXY is set"));
        assert!(out.contains("Write only in the working directory"));
        assert!(out.contains("expect 401 or 404"));
        assert!(out.contains("Do not guess paths."));
    }

    #[test]
    fn tells_the_agent_to_install_a_missing_tool() {
        let out = render(&input(None));
        assert!(out.contains("If a tool the work needs is missing, install it"));
        assert!(!out.contains("do not try to install"));
    }

    #[test]
    fn setup_failure_is_named_only_when_it_failed() {
        assert!(!render(&input(None)).contains("Setup failed"));
        let mut b = input(None);
        b.setup_exit = Some(3);
        let out = render(&b);
        assert!(out.contains("Setup failed (exit 3)"));
        assert!(out.contains(".moor/setup.log"));
        assert!(out.contains(".moor/reports/<kind>.md"));
    }

    #[test]
    fn repo_notes_follow_the_issue_under_their_own_heading_verbatim() {
        let mut b = input(Some("x is broken"));
        b.repo_notes = Some("Start the emulator:\n  make emu\n");
        let out = render(&b);
        let issue = out.find("Issue: x is broken").unwrap();
        let heading = "## This project's Box\n\nFrom the repo's config:\n\nStart the emulator:\n  make emu\n\n";
        assert!(out.contains(heading), "{out}");
        let head = out.find(heading).unwrap();
        assert!(issue < head, "{out}");
        assert!(head < out.find("You are running inside the machine").unwrap());
    }

    #[test]
    fn no_repo_notes_no_heading() {
        let out = render(&input(None));
        assert!(!out.contains("## This project's Box"), "{out}");
    }

    #[test]
    fn states_cpus_and_memory_when_known() {
        let mut b = input(None);
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
        assert!(!render(&input(None)).contains("vCPU"));
    }

    #[test]
    fn says_to_check_before_calling_a_tool_missing() {
        let out = render(&input(None));
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
        let mut b = input(Some("i"));
        b.setup_exit = Some(1);
        let out = render(&b);
        assert!(!out.contains("/home/"));
        assert!(!out.contains("moor run"));
        assert!(!out.contains("moor report"));
        assert!(!out.to_lowercase().contains("token"));
    }
}
