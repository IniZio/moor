//! The brief the agent keeps as standing instructions: moor's contract for its situation, then
//! the facts of its Box. An agent place still reads its own text until its slice moves it.

use std::fmt::Write;

use moor_core::contract::{Brief, Contract, Situation, SkillEntry, ARTIFACTS_DIR, REPORTS_DIR};
use moor_core::seam::engine::{HeldRepo, RunLaunch};
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
    /// The contract a work Box reads.
    pub(super) contract: &'a Contract,
    /// The run this launch serves; none for a task's Box.
    pub(super) run: Option<&'a RunLaunch>,
    /// Whether the agent has the moor MCP server, so its tools are listed.
    pub(super) moor_tools: bool,
    /// The built-in skills written into the Box for this launch.
    pub(super) skills: &'a [SkillEntry],
}

/// The situations of a work Box's launch.
pub(super) const fn situations(run: bool) -> &'static [Situation] {
    if run {
        &[Situation::Every, Situation::Box, Situation::Run]
    } else {
        &[Situation::Every, Situation::Box]
    }
}

/// The brief text for a Box.
pub(super) fn render(b: &BriefInput) -> String {
    if b.place {
        return place(b);
    }
    let mut out = Brief {
        contract: b.contract,
        situations: situations(b.run.is_some()),
        moor_tools: b.moor_tools,
        skills: b.skills,
    }
    .render();
    facts(b, &mut out);
    out
}

/// The facts of a work Box, after the contract.
fn facts(b: &BriefInput, out: &mut String) {
    let (name, branch) = (b.name, b.branch);
    _ = write!(out, "# Box {name}\n\n");
    if let Some(i) = b.issue {
        _ = write!(out, "Issue: {i}\n\n");
    }
    project_facts(b, out);
    _ = write!(
        out,
        "## This Box\n\n\
         - The checkout is on branch `{branch}`. Its push line: \
         `git push origin HEAD:refs/heads/{branch}`.\n\
         - The checkout is a partial clone with full history. git fetches file contents through \
         moor's Proxy when a command first needs them.\n\
         - The artifacts folder is `{ARTIFACTS_DIR}/` and the reports folder is \
         `{REPORTS_DIR}/`, both in your working directory.\n"
    );
    if let Some(link) = b.run.and_then(|r| r.link.as_deref()) {
        _ = writeln!(out, "- This run's link: {link}");
    }
    out.push('\n');
    repos::section(b.repos, branch, out);
    if let Some(n) = b.setup_exit {
        setup_failed(n, out);
    }
    out.push_str(GH_RECIPE);
}

/// An agent place's brief.
fn place(b: &BriefInput) -> String {
    let name = b.name;
    let mut out = format!("# Box {name}\n\n");
    if let Some(i) = b.issue {
        _ = write!(out, "Issue: {i}\n\n");
    }
    project_facts(b, &mut out);
    out.push_str(PLACE);
    out.push_str(
        "Reports are how you show work to a human: a spec, a checkpoint, a result. \
         Kind is a free-form label (e.g. `spec`, `result`). To present a Report, write it as \
         a Markdown file at `.moor/reports/<kind>.md` in your working directory; do not \
         commit it. Writing again with the same kind adds a new version. Name that path in \
         your final message. A Report never means the Box is finished; keep working \
         until the work is done, and do not stop after reporting.\n\n",
    );
    out.push_str(GH_RECIPE);
    out.push_str(GH_PLACE);
    out
}

/// How an agent place works: no checkout, and the repos of each task named in its first message.
const PLACE: &str = "You are running inside the machine, in an agent place: it holds no \
    checkout and no branch, and waits for a task. Each task's first message starts with a \
    `Repos:` line naming the GitHub repos it is about, then a blank line, then the person's \
    message. Read those repos through GitHub: `gh api`, or \
    `git clone --filter=blob:none https://github.com/OWNER/NAME.git /tmp/NAME`. Requests for \
    the repos the task names carry the owner's credential through moor's Proxy. When \
    HTTPS_PROXY is set, do not unset or bypass it. Write only \
    in /tmp, your own tool state and the tools you install. Answer in your final message.\n\n\
    When the task needs code run, start a work Box with `start_box` and run commands in it \
    with `exec_in_box`; they run in the Box's checkout. When the work does not need the repo's \
    setup (reading code, a small edit), pass `no_setup` to `start_box` so the Box is ready \
    sooner. Save a file the person should see (a \
    screenshot, test output) in the work Box's `.moor/artifacts/`, not in your own folder; \
    moor collects it within about 30 seconds and when the Box closes, and it stays on the task \
    after the Box is gone. Do not commit it. Name the files you saved in your final message. \
    Commit your work in the Box and push it with `git push origin HEAD`. Then open the pull \
    request from this place with gh, installing gh with the recipe below when \
    `command -v gh` finds none, and end your final message with its URL. Then close the Box \
    with `close_box`, which refuses unpushed work. A later work Box of this task starts on \
    the task's branch with the commits pushed before, so its push adds to the same pull \
    request: open a new one only when none is open. One work Box of a task is open at a \
    time.\n\n\
    You may receive follow-up messages (steers) typed into this session.\n\n\
    Decide on your own when you reasonably can, and ask a question only when the choice is \
    the owner's.\n\n";

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

/// How to get gh when the Box image lacks it: a pinned release, checked against its sha256.
const GH_RECIPE: &str = r#"When `command -v gh` finds nothing, install gh into `~/.local/bin` with this, then run gh as usual:

```sh
v=2.105.0; f=gh_${v}_linux_$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').tar.gz
curl -fsSL --retry 3 -o /tmp/$f https://github.com/cli/cli/releases/download/v$v/$f
printf '%s\n' \
  '79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f  gh_2.105.0_linux_amd64.tar.gz' \
  'b1278c75107423d818f1937fd92cafc9e5e9b606631568f68d7fb816f5885a02  gh_2.105.0_linux_arm64.tar.gz' \
  | grep " $f" | (cd /tmp && sha256sum -c --quiet) \
  && tar -xzf /tmp/$f -C /tmp && install -D /tmp/${f%.tar.gz}/bin/gh ~/.local/bin/gh
```

"#;

/// How gh reaches GitHub from an agent place, and what the Proxy withholds there.
const GH_PLACE: &str = "gh reaches GitHub as the Box owner through moor's Proxy. gh's environment holds only a placeholder, and the Proxy puts the owner's credential on each request its rules allow. Do not run `gh auth login` or change gh's credentials or config. Reads of any repo carry the credential; writes are limited to the repos this Box or task names. A 401 or 404 on a write (a push, a pull request, a comment) to any other repo means the Proxy withheld the credential. Posting a comment on an issue or pull request (`gh pr comment`, `gh issue comment`, or `POST /repos/{owner}/{repo}/issues/<number>/comments`) is withheld unless this Box's config asks for it, so a 401 there is expected: do not retry or look for a way around it, and put what you would have said in your report.\n";

#[cfg(test)]
mod tests;
