//! moor's agent contract: the distro file built into the binary, its skills, and the text one
//! launch reads. See docs/specs/agent-contract.md.

use std::fmt::Write;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};

mod parse;
#[cfg(test)]
mod tests;

/// The distro's contract file, the one source of every copy.
pub const SOURCE: &str = include_str!("../../../agents/distro/contract.md");

/// The folder of a Box's working directory whose top-level files moor collects for the person.
pub const ARTIFACTS_DIR: &str = ".moor/artifacts";

/// The folder of a Box's working directory whose `<kind>.md` files moor shows as reports.
pub const REPORTS_DIR: &str = ".moor/reports";

/// The lock file that pins [`SOURCE`]'s version to its sha256.
pub const LOCK: &str = include_str!("../../../agents/distro/contract.lock");

/// Where an agent stands, from what moor knows at launch. Never a harness or a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Situation {
    /// Every agent moor launches.
    Every,
    /// An agent working in a run's Box, which holds a checkout.
    Box,
    /// An agent working through a run's gates.
    Run,
    /// An agent in an agent place: no checkout, a task's repos read through GitHub, and work
    /// Boxes it starts, which run no agent.
    Place,
}

impl Situation {
    /// The section heading key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Every => "every",
            Self::Box => "box",
            Self::Run => "run",
            Self::Place => "place",
        }
    }

    /// The situation a heading key names.
    fn of(key: &str) -> Option<Self> {
        [Self::Every, Self::Box, Self::Run, Self::Place]
            .into_iter()
            .find(|s| s.key() == key)
    }
}

/// One statement: its key and its text, reason included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statement {
    /// Unique, lower case and hyphenated.
    pub key: &'static str,
    /// The act, then its reason.
    pub text: &'static str,
}

/// The contract, parsed from [`SOURCE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// The `Version N` under the title.
    pub version: u32,
    /// The title line, without its `# `.
    pub title: &'static str,
    /// Who the agent works for, and precedence.
    pub preamble: &'static str,
    /// Each situation's statements, in file order.
    pub sections: Vec<(Situation, Vec<Statement>)>,
}

/// What a launch records of the contract it read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The contract version.
    pub version: u32,
    /// The sha256 of the contract file, in hex.
    pub sha256: String,
}

/// The parsed built-in contract.
static BUILT_IN: LazyLock<Result<Contract, String>> = LazyLock::new(|| parse::contract(SOURCE));

/// The built-in contract.
///
/// # Errors
/// Why the distro file does not parse; a test proves it does.
pub fn contract() -> Result<&'static Contract, &'static str> {
    BUILT_IN.as_ref().map_err(String::as_str)
}

/// The sha256 of `text`, in hex.
#[must_use]
pub fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::new(), |mut out, b| {
            _ = write!(out, "{b:02x}");
            out
        })
}

impl Contract {
    /// The statements of `situation`; none when the file has no such section.
    #[must_use]
    pub fn statements(&self, situation: Situation) -> &[Statement] {
        self.sections
            .iter()
            .find(|(s, _)| *s == situation)
            .map_or(&[], |(_, st)| st.as_slice())
    }

    /// What a launch records: this version and the file's sha256.
    #[must_use]
    pub fn stamp(&self) -> Stamp {
        Stamp {
            version: self.version,
            sha256: sha256(SOURCE),
        }
    }
}

/// A built-in skill, from its SKILL.md front matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Its name, which starts with `moor-`.
    pub name: &'static str,
    /// When to use it, from its front matter.
    pub description: &'static str,
    /// The whole SKILL.md file.
    pub file: &'static str,
    /// The situations whose launches list it.
    pub situations: &'static [Situation],
}

/// The distro's skill files and the situations that list each.
const SKILL_FILES: &[(&str, &[Situation])] = &[(
    include_str!("../../../agents/distro/skills/moor-attach-files/SKILL.md"),
    &[Situation::Box],
)];

/// The parsed built-in skills.
static SKILLS: LazyLock<Result<Vec<Skill>, String>> = LazyLock::new(|| {
    SKILL_FILES
        .iter()
        .map(|(file, situations)| parse::skill(file, situations))
        .collect()
});

/// The built-in skills.
///
/// # Errors
/// Why a skill's front matter does not parse; a test proves each does.
pub fn skills() -> Result<&'static [Skill], &'static str> {
    SKILLS.as_ref().map(Vec::as_slice).map_err(String::as_str)
}

/// The tools of the Box-facing moor MCP server and the situation whose statements name each. A
/// test in moor-gate holds this list to the server's routers.
pub const TOOLS: &[(&str, Situation)] = &[
    ("report_output", Situation::Every),
    ("report_step", Situation::Run),
    ("start_box", Situation::Place),
    ("exec_in_box", Situation::Place),
    ("exec_wait", Situation::Place),
    ("close_box", Situation::Place),
];

/// A skill as one launch lists it: where the adapter put its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// The skill's name.
    pub name: &'static str,
    /// The skill's description.
    pub description: &'static str,
    /// Where its SKILL.md is in the Box.
    pub path: String,
}

/// What one launch reads, before its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief<'a> {
    /// The contract.
    pub contract: &'a Contract,
    /// The launch's situations, [`Situation::Every`] first.
    pub situations: &'a [Situation],
    /// Whether the launch has the moor MCP server, so its tools are listed.
    pub moor_tools: bool,
    /// The skills the launch lists.
    pub skills: &'a [SkillEntry],
}

impl Brief<'_> {
    /// The tools this launch's situations name.
    fn tools(&self) -> Vec<&'static str> {
        TOOLS
            .iter()
            .filter(|(_, s)| self.situations.contains(s))
            .map(|(name, _)| *name)
            .collect()
    }

    /// The contract text: title, version and preamble, each situation's section, then the
    /// generated tool and skill blocks. Facts follow it, written by the caller.
    #[must_use]
    pub fn render(&self) -> String {
        let c = self.contract;
        let mut out = format!(
            "# {}\n\nVersion {}\n\n{}\n\n",
            c.title, c.version, c.preamble
        );
        for &s in self.situations {
            _ = write!(out, "## {}\n\n", s.key());
            for st in c.statements(s) {
                _ = writeln!(out, "- [{}] {}", st.key, st.text);
            }
            out.push('\n');
        }
        let tools = self.tools();
        if self.moor_tools && !tools.is_empty() {
            _ = write!(
                out,
                "## moor tools\n\nOn the MCP server named moor: {}.\n\n",
                tools.join(", ")
            );
        }
        if !self.skills.is_empty() {
            out.push_str("## moor skills\n\n");
            for s in self.skills {
                _ = writeln!(
                    out,
                    "- {}: {} Its file: `{}`",
                    s.name, s.description, s.path
                );
            }
            out.push('\n');
        }
        out
    }
}
