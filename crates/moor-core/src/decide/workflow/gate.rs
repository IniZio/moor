//! The code gate: what a step's report must show before the run moves on.

use crate::artifact::ArtifactFile;
use crate::workflow::{
    EvidenceName, Step, StepKind, StepReport, StepResult, EVIDENCE_MAX, NOTE_MAX,
};

/// Whether `report` of `step` passes, given `index`, the top-level regular files of the Box's
/// artifacts folder. The refusal lists every problem at once, so the agent fixes them in one go.
///
/// - A note is at most [`NOTE_MAX`] bytes.
/// - A report of `fail` passes with a note that is not empty, whatever the step's evidence.
/// - A work step's report of `ok` passes when each name of its evidence matches a file, flat or
///   by `prefix*`, and every file it matches is not empty and at most [`EVIDENCE_MAX`].
/// - A deliver step's report passes with a note that is not empty: the run's result.
///
/// # Errors
/// What is missing, too small or too large, one line each. A step that moor runs itself has no
/// report to pass, and is refused as such.
pub fn code_gate(
    step: &Step,
    report: &StepReport,
    index: &[ArtifactFile],
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    if report.note.len() > NOTE_MAX {
        problems.push(format!(
            "the note is {} bytes; at most {NOTE_MAX} are kept",
            report.note.len()
        ));
    }
    let said_why = !report.note.trim().is_empty();
    match (&step.kind, report.result) {
        (StepKind::Check { .. } | StepKind::Approve { .. }, _) => {
            problems.push(format!(
                "step {} is not reported by the agent; moor runs it",
                step.id
            ));
        }
        (_, StepResult::Fail) => {
            if !said_why {
                problems.push("a fail report needs a note that says why".to_owned());
            }
        }
        (StepKind::Deliver { .. }, StepResult::Ok) => {
            if !said_why {
                problems
                    .push("a deliver report needs a note: it becomes the run's result".to_owned());
            }
        }
        (StepKind::Work { evidence, .. }, StepResult::Ok) => {
            for name in evidence.names() {
                problems.extend(evidence_problems(name, index));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// What is wrong with the files `name` asks for.
fn evidence_problems(name: &EvidenceName, index: &[ArtifactFile]) -> Vec<String> {
    let text = name.as_str();
    let matching: Vec<&ArtifactFile> = match text.strip_suffix('*') {
        Some(prefix) => index
            .iter()
            .filter(|f| f.name.starts_with(prefix))
            .collect(),
        None => index.iter().filter(|f| f.name == text).collect(),
    };
    if matching.is_empty() {
        return vec![format!("no file matches `{text}` in .moor/artifacts/")];
    }
    matching
        .into_iter()
        .filter_map(|f| match f.size {
            0 => Some(format!("`{}` is empty", f.name)),
            n if n > EVIDENCE_MAX => Some(format!(
                "`{}` is {n} bytes; at most {EVIDENCE_MAX} (25 MiB) are accepted",
                f.name
            )),
            _ => None,
        })
        .collect()
}
