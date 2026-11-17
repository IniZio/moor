//! The wire form of a plan and what can be wrong with it.

use schemars::JsonSchema;

use super::{Step, StepKind, Tries};

/// A plan as written: the JSON a model sends and `list_workflows` shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDoc {
    /// The steps, 1 to 20. The first one is where a run starts. Every step must lead on to a
    /// `deliver` step, and a `deliver` step ends the run.
    pub steps: Vec<StepDoc>,
    /// The hash of the prompt the plan was made from. Leave it out for a plan you wrote for the
    /// prompt you send. Send back the value `list_workflows` showed only to say the steps were
    /// not reviewed against a changed prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_from: Option<String>,
    /// Whether a person changed the plan after it was made. Leave it out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub edited: bool,
}

/// One step as written. Which fields a step takes depends on its `kind`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepDoc {
    /// 1-32 characters of a-z, 0-9 and `-`, starting with a letter. Unique in the plan. Keep the
    /// id of a step that did not change when you rewrite a plan.
    pub id: String,
    /// A short name, at most 120 bytes.
    pub title: String,
    /// `work` (the agent does something in the Box and leaves evidence files), `check` (moor
    /// runs `command` in the Box and reads its exit code), `approve` (the run waits for the
    /// owner to say yes or no) or `deliver` (the agent delivers, and its note is the run's
    /// result; it ends the run).
    pub kind: String,
    /// How often a run may enter this step, 1 to 5; 1 when left out. A step entered more often
    /// follows its `fail` edge at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tries: Option<u64>,
    /// `work` and `deliver`: what to do, at most 4 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// `work` and `deliver`: how the agent knows it is done, at most 4 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<String>,
    /// `work`: the files the agent leaves under `.moor/artifacts/`, one or more. Each is a flat
    /// file name such as `report.md`, or a prefix ending in `*` such as `before-*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<String>>,
    /// `check`: the shell command, at most 4 KiB. Exit 0 passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `approve`: what the owner is asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// `work`, `check` and `approve`: the id of the step after a pass. A `deliver` step has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<String>,
    /// `work`, `check` and `approve`: the id of the step after a fail; the run ends failed when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail: Option<String>,
}

/// Serde's skip test for a flag.
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
const fn is_false(b: &bool) -> bool {
    !*b
}

/// One thing wrong with a plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The text is not a plan at all: not JSON, a field of the wrong type, or an unknown field.
    #[error("the plan is not valid: {0}")]
    Json(String),
    /// No steps.
    #[error("a plan needs at least one step")]
    NoSteps,
    /// More than 20 steps.
    #[error("a plan has {0} steps; use at most 20")]
    TooManySteps(usize),
    /// An id that is not 1-32 of `[a-z0-9-]` starting with a letter.
    #[error("{step}: invalid step id; use 1-32 lowercase letters, digits or '-', starting with a letter")]
    BadId {
        /// The step, by place.
        step: String,
    },
    /// Two steps share an id.
    #[error("step id `{0}` is used twice")]
    DuplicateId(String),
    /// A title that is empty or over 120 bytes.
    #[error("{step}: the title must be 1 to 120 bytes, not {len}")]
    Title {
        /// The step.
        step: String,
        /// Its length.
        len: usize,
    },
    /// A `max_tries` outside 1 to 5.
    #[error("{step}: max_tries must be 1 to 5, not {got}")]
    MaxTries {
        /// The step.
        step: String,
        /// What was written.
        got: u64,
    },
    /// A kind that is none of the four.
    #[error("{step}: unknown kind `{kind}`; use work, check, approve or deliver")]
    UnknownKind {
        /// The step.
        step: String,
        /// What was written.
        kind: String,
    },
    /// A field the kind needs is missing or empty.
    #[error("{step}: a {kind} step needs `{field}`")]
    Missing {
        /// The step.
        step: String,
        /// Its kind.
        kind: &'static str,
        /// The field.
        field: &'static str,
    },
    /// A field the kind does not take.
    #[error("{step}: a {kind} step takes no `{field}`")]
    Unexpected {
        /// The step.
        step: String,
        /// Its kind.
        kind: &'static str,
        /// The field.
        field: &'static str,
    },
    /// Text over 4 KiB.
    #[error("{step}: `{field}` is {len} bytes; use at most 4096")]
    TooLong {
        /// The step.
        step: String,
        /// The field.
        field: &'static str,
        /// Its length.
        len: usize,
    },
    /// An evidence name that is neither an artifact name nor a `prefix*`.
    #[error("{step}: evidence `{name}` is not a flat file name or a `prefix*`")]
    BadEvidence {
        /// The step.
        step: String,
        /// What was written.
        name: String,
    },
    /// An edge to a step that is not in the plan.
    #[error("{step}: `{edge}` names `{target}`, which is not a step of the plan")]
    UnknownTarget {
        /// The step.
        step: String,
        /// `ok` or `fail`.
        edge: &'static str,
        /// What was written.
        target: String,
    },
    /// The plan has no `deliver` step, so no run could end delivered.
    #[error("a plan needs a deliver step")]
    NoDeliver,
    /// A step from which no `deliver` step can be reached.
    #[error("{step}: no deliver step can be reached from here, so a run could never finish")]
    Trapped {
        /// The step.
        step: String,
    },
    /// A `made_from` that is not 64 hex digits.
    #[error("made_from must be 64 hex digits")]
    MadeFrom,
}

/// `step` as written.
pub(super) fn doc_of(step: &Step) -> StepDoc {
    let mut doc = StepDoc {
        id: step.id.to_string(),
        title: step.title.clone(),
        kind: step.kind.name().to_owned(),
        max_tries: (step.max_tries != Tries::ONE).then(|| u64::from(step.max_tries.get())),
        instructions: None,
        done_when: None,
        evidence: None,
        command: None,
        question: None,
        ok: None,
        fail: None,
    };
    if let Some(next) = step.kind.next() {
        doc.ok = Some(next.ok.to_string());
        doc.fail = next.fail.as_ref().map(ToString::to_string);
    }
    match &step.kind {
        StepKind::Work {
            instructions,
            done_when,
            evidence,
            ..
        } => {
            doc.instructions = Some(instructions.clone());
            doc.done_when = Some(done_when.clone());
            doc.evidence = Some(evidence.names().map(|n| n.as_str().to_owned()).collect());
        }
        StepKind::Check { command, .. } => doc.command = Some(command.clone()),
        StepKind::Approve { question, .. } => doc.question = Some(question.clone()),
        StepKind::Deliver {
            instructions,
            done_when,
        } => {
            doc.instructions = Some(instructions.clone());
            doc.done_when = Some(done_when.clone());
        }
    }
    doc
}
