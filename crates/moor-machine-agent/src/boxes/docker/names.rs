//! Every Docker object a Box owns is named from its slug, here and nowhere else, so the provider
//! can remove exactly what it made and nothing a neighbour on the host owns.

use crate::boxes::Slug;

/// The names of one Box's containers and volumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Names {
    /// The agent place's container.
    pub(super) agent: String,
    /// The work place's container.
    pub(super) work: String,
    /// The work tree both places mount.
    pub(super) tree: String,
    /// `/run/moor`, both places.
    pub(super) run: String,
    /// The agent place's home.
    pub(super) agent_home: String,
    /// The work place's home.
    pub(super) work_home: String,
    /// The work place's `/var/lib/docker`.
    pub(super) docker: String,
    /// `~/.agents`, in both places, so skills the work place installs reach Claude.
    pub(super) agents: String,
    /// `~/.claude/skills`, in both places.
    pub(super) skills: String,
    /// `/tmp`, in both places, so a script Claude writes in one runs in the other.
    pub(super) tmp: String,
}

impl Names {
    /// The names for `slug`.
    pub(super) fn of(slug: &Slug) -> Self {
        let s = slug.as_str();
        let name = |part: &str| format!("{s}-{part}");
        Self {
            agent: name("agent"),
            work: name("work"),
            tree: name("tree"),
            run: name("run"),
            agent_home: name("agent-home"),
            work_home: name("work-home"),
            docker: name("docker"),
            agents: name("agents"),
            skills: name("skills"),
            tmp: name("tmp"),
        }
    }

    /// Every volume the Box owns.
    pub(super) fn volumes(&self) -> [&str; 8] {
        [
            &self.tree,
            &self.run,
            &self.agent_home,
            &self.work_home,
            &self.docker,
            &self.agents,
            &self.skills,
            &self.tmp,
        ]
    }
}
