//! What a Box that needs a person asks of them: the card a front shows and the answers it takes.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::view::{AgentView, ApprovalView};

/// What the agent asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedKind {
    /// It wants to run a tool, such as a command.
    Permission,
    /// It asks a question.
    Question,
    /// A workflow run waits at an approve step for its owner's yes or no.
    Approve,
}

/// What picking an option does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Allow this one use.
    AllowOnce,
    /// Allow it from now on, for this Box.
    AllowBox,
    /// Refuse it.
    Deny,
    /// An answer to a question.
    Choice,
}

/// The workflow run an approve card answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approve {
    /// The workflow's name.
    pub workflow: String,
    /// The run's id.
    pub run: i64,
    /// The title of the approve step the run waits at.
    pub step: String,
}

/// One button of the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    /// The 1-based menu number to send.
    pub n: u32,
    /// The agent's own words for it.
    pub label: String,
    /// What it does.
    pub role: Role,
}

/// The card of a Box that needs a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    /// Names this prompt. An answer carries it, so it never lands on a later prompt.
    pub id: String,
    /// A permission or a question.
    pub kind: NeedKind,
    /// What is asked.
    pub question: String,
    /// What the prompt shows above its menu, such as the command; quoted on the card.
    pub detail: Option<String>,
    /// The buttons, in menu order.
    pub picks: Vec<Pick>,
    /// The menu number of the entry that opens a text field, when the prompt has one. Pick it
    /// together with the text to answer in the person's own words.
    pub other: Option<u32>,
    /// For an approve card: the run its yes or no goes to. It has no picks.
    #[serde(default)]
    pub approve: Option<Approve>,
}

/// The name of the prompt that asks `question` with `detail` and `options`.
#[must_use]
pub fn ask_id(question: Option<&str>, detail: Option<&str>, options: &[String]) -> String {
    let mut hash = Sha256::new();
    for part in [question.unwrap_or_default(), detail.unwrap_or_default()]
        .into_iter()
        .chain(options.iter().map(String::as_str))
    {
        hash.update(part.len().to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()
        .iter()
        .take(8)
        .fold(String::new(), |mut id, b| {
            let _ = write!(id, "{b:02x}");
            id
        })
}

/// What a permission menu entry does, from the words the agent puts on it.
fn role_of(label: &str) -> Role {
    let l = label.trim().to_lowercase();
    if l.starts_with("no") && !l.starts_with("not ") {
        Role::Deny
    } else if l.starts_with("yes") && l.len() > 3 {
        Role::AllowBox
    } else {
        Role::AllowOnce
    }
}

/// The card for an agent that waits, when it has a menu to answer. Without options there is
/// nothing the portal can pick, so there is no card.
#[must_use]
pub fn needs_of(agent: &AgentView) -> Option<Needs> {
    if agent.state != "needs_input" || agent.options.is_empty() {
        return None;
    }
    let kind = if agent.kind.as_deref() == Some("permission") {
        NeedKind::Permission
    } else {
        NeedKind::Question
    };
    let picks = agent
        .options
        .iter()
        .zip(1u32..)
        .map(|(label, n)| Pick {
            n,
            label: label.clone(),
            role: match kind {
                NeedKind::Permission => role_of(label),
                NeedKind::Question | NeedKind::Approve => Role::Choice,
            },
        })
        .collect();
    Some(Needs {
        id: ask_id(
            agent.question.as_deref(),
            agent.detail.as_deref(),
            &agent.options,
        ),
        kind,
        question: agent
            .question
            .clone()
            .unwrap_or_else(|| "The agent needs your input".to_owned()),
        detail: agent.detail.clone(),
        other: (kind == NeedKind::Question)
            .then(|| u32::try_from(agent.options.len() + 1).ok())
            .flatten(),
        picks,
        approve: None,
    })
}

/// The card for a run that waits at an approve step: the step's question, to answer yes or no.
#[must_use]
pub fn approve_needs(approval: &ApprovalView) -> Needs {
    Needs {
        id: ask_id(
            Some(&approval.question),
            Some(&format!("{}#{}", approval.workflow, approval.run)),
            &[],
        ),
        kind: NeedKind::Approve,
        question: approval.question.clone(),
        detail: None,
        picks: Vec::new(),
        other: None,
        approve: Some(Approve {
            workflow: approval.workflow.clone(),
            run: approval.run,
            step: approval.step.clone(),
        }),
    }
}
