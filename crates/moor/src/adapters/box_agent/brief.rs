//! The brief the agent keeps as standing instructions: moor's contract for its situation, then
//! the facts of its Box or agent place.

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
    /// The contract the launch reads.
    pub(super) contract: &'a Contract,
    /// The run this launch serves; none for an agent place.
    pub(super) run: Option<&'a RunLaunch>,
    /// Whether the agent has the moor MCP server, so its tools are listed.
    pub(super) moor_tools: bool,
    /// The built-in skills written into the Box for this launch.
    pub(super) skills: &'a [SkillEntry],
}

/// The situations of a launch: an agent place's, or a work Box's (a run's when `run`).
pub(super) const fn situations(place: bool, run: bool) -> &'static [Situation] {
    match (place, run) {
        (true, _) => &[Situation::Every, Situation::Place],
        (false, true) => &[Situation::Every, Situation::Box, Situation::Run],
        (false, false) => &[Situation::Every, Situation::Box],
    }
}

/// The brief text for a Box or an agent place.
pub(super) fn render(b: &BriefInput) -> String {
    let mut out = Brief {
        contract: b.contract,
        situations: situations(b.place, b.run.is_some()),
        moor_tools: b.moor_tools,
        skills: b.skills,
    }
    .render();
    if b.place {
        place_facts(b, &mut out);
    } else {
        facts(b, &mut out);
    }
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
}

/// The facts of an agent place, after the contract.
fn place_facts(b: &BriefInput, out: &mut String) {
    _ = write!(out, "# Box {}\n\n", b.name);
    project_facts(b, out);
    _ = write!(
        out,
        "## This place\n\n\
         - It holds no checkout. Read a repo with `gh api`, or \
         `git clone --filter=blob:none https://github.com/OWNER/NAME.git /tmp/NAME`.\n\
         - Write only in /tmp, your own tool state and the tools you install. When HTTPS_PROXY \
         is set, do not unset or bypass it.\n\
         - A work Box's checkout is on the task's branch. Its artifacts folder is \
         `{ARTIFACTS_DIR}/` in its working directory.\n\n"
    );
}

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


#[cfg(test)]
mod tests;
