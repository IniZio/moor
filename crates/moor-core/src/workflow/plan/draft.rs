//! What the agent is told when it writes a plan instead of following one: a short Box that drafts
//! a workflow's plan, and a one-off that proposes its plan and waits for approval first.

use super::PlanDoc;
use crate::gate::GatesDoc;

/// The evidence file a drafted or proposed plan is left in.
pub const PLAN_FILE: &str = "plan.json";

/// What a plan-first agent asks once its plan is written. A card asking exactly this is a plan's.
pub const PLAN_QUESTION: &str = "Approve this plan?";

/// The answer that lets a plan-first agent carry out its plan.
pub const APPROVE_PLAN: &str = "Approve plan";

/// The answer that sends a plan-first agent back to its plan.
pub const REVISE_PLAN: &str = "Revise";

/// Where the plan-first part of a prompt starts, after the person's own words.
const PLAN_FIRST_HEAD: &str = "\n\nPlan first. ";

/// How to write a plan, with the schema `Plan::parse` reads.
fn how_to_write() -> String {
    let schema = schemars::schema_for!(PlanDoc);
    let schema = serde_json::to_string(&schema).unwrap_or_default();
    format!(
        "A plan is one step per finish line. Use work steps for what the agent does, each with \
         the evidence files it leaves in .moor/artifacts/; check steps for anything a command \
         can decide; approve steps where the owner must say yes first; and a final deliver step \
         for the pull request, message or file the prompt asks for. Every step must lead on to \
         the deliver step. Leave out made_from and edited. The plan is JSON that matches this \
         schema:\n{schema}"
    )
}

/// How to write gates in place of a plan, with the schema `Gates::from_json` reads.
fn how_to_gate() -> String {
    let schema = schemars::schema_for!(GatesDoc);
    let schema = serde_json::to_string(&schema).unwrap_or_default();
    format!(
        "Write gates instead of a plan when the prompt has each run pass named gates in order \
         (it says moor holds the run open until its gates pass, or asks for report_step for a \
         gate). Then the file is an object with `gates` and `outcomes` and no `steps`. Name \
         one gate for each gate the prompt names, in its order, each with the evidence files the \
         prompt names for it, and no file it does not name. An evidence name is a flat file \
         name, or a prefix ending in * that covers names with a number in them, such as pr-* \
         for pr-<n>-check.log and pr-<n>-summary.json; a * anywhere else is refused. For \
         everything the prompt says moor checks itself, or says must hold on GitHub or \
         in a file before the gate passes, add a check: a shell command moor runs in the run's \
         Box, at the work tree's root, once the evidence is in, that exits 0 only when it holds, \
         with `says` as one plain sentence. Write checks with sh, git, gh, grep, sed and awk: \
         the run's Box may have no python3, jq or node, so read JSON with gh's --jq or grep. \
         Evidence files are under .moor/artifacts/. A run fired \
         by a pull request has MOOR_PULL_REPO, MOOR_PULL_NUMBER, MOOR_PULL_HEAD_SHA and \
         MOOR_PULL_URL in its env, with the env of the workflow's config; gh can read GitHub. \
         When a run can end in different verdicts, such as check passed or failed, nothing to \
         do, or needs a person, write them as outcomes: a name of up to 40 characters, one \
         sentence, a tone (good, bad or plain) and the checks that decide it, using the commands \
         the config gives for that where it has them. moor tries the outcomes in order once \
         every gate passed, and the first whose checks all pass is the run's verdict; only the \
         last one may have no check. Mark an outcome anytime when it can end a run before its \
         gates pass, such as nothing to do. Leave out made_from. The gates are JSON that \
         matches this schema:\n{schema}"
    )
}

/// How a draft prompt opens, so a front can tell a drafting Box from a person's own.
const DRAFT_HEAD: &str = "Draft a plan for this workflow prompt.";

/// Whether `text` is a prompt `draft_prompt` made.
#[must_use]
pub fn is_draft_prompt(text: &str) -> bool {
    text.starts_with(DRAFT_HEAD)
}

/// What a short Box that reads a workflow's prompt is asked, for the workflow prompt `prompt`
/// and the config `config` its runs' Boxes get: a plan, or gates when the prompt names them.
#[must_use]
pub fn draft_prompt(prompt: &str, config: Option<&str>) -> String {
    let config = config
        .filter(|c| !c.trim().is_empty())
        .map(|c| {
            format!(
                "Each run's Box gets this config, laid over the repo's:\n```yaml\n{}\n```\n\n",
                c.trim()
            )
        })
        .unwrap_or_default();
    format!(
        "{DRAFT_HEAD} Only read: change no file, branch or pull request.\n\nThe workflow prompt:\n\"\"\"\n{}\n\"\"\"\n\n{config}Read the repos to see what the \
         prompt needs, then write moor's reading of it: a plan, or gates. Each run starts a new Box \
         on its own branch, so name no branch, Box or folder of this one.\n\n{}\n\n{}\n\nWrite \
         the plan or the gates as .moor/artifacts/{PLAN_FILE} and nothing else, then end your turn.",
        prompt.trim(),
        how_to_write(),
        how_to_gate()
    )
}

/// What a plan-first one-off's agent is asked: `prompt`, then to propose a plan, ask for
/// approval, and carry the plan out once approved.
#[must_use]
pub fn plan_first_prompt(prompt: &str) -> String {
    format!(
        "{}{PLAN_FIRST_HEAD}Before you change anything, read the repos and write a plan for the \
         work above as .moor/artifacts/{PLAN_FILE}. Change no other file until the plan is \
         approved.\n\n{}\n\nThen ask for approval with the AskUserQuestion tool: one question, \
         exactly \"{PLAN_QUESTION}\", with the two options \"{APPROVE_PLAN}\" and \"{REVISE_PLAN}\". \
         Wait for the answer.\n- On \"{APPROVE_PLAN}\", carry out the steps in order. Do each work \
         step and leave its evidence files. Run each check step's command yourself and follow \
         its ok or fail edge. At each approve step, ask its question with AskUserQuestion and \
         follow the answer. End with the deliver step.\n- On \"{REVISE_PLAN}\" or any other \
         answer, change the plan as the answer says, write it again, and ask again the same way.",
        prompt.trim_end(),
        how_to_write()
    )
}

/// The person's own words of a prompt `plan_first_prompt` made; none for any other prompt.
#[must_use]
pub fn split_plan_first(text: &str) -> Option<&str> {
    text.find(PLAN_FIRST_HEAD).map(|at| text.split_at(at).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_first_prompt_keeps_the_persons_words_first_and_gives_them_back() {
        let asked = plan_first_prompt("Fix the login typo.\n");
        assert!(asked.starts_with("Fix the login typo.\n\nPlan first. "));
        assert!(asked.contains(PLAN_QUESTION) && asked.contains(".moor/artifacts/plan.json"));
        assert_eq!(split_plan_first(&asked), Some("Fix the login typo."));
        assert_eq!(split_plan_first("Fix the login typo."), None);
    }

    #[test]
    fn a_draft_prompt_quotes_the_workflow_prompt_and_the_plan_schema() {
        let asked = draft_prompt("Bump the deps.", None);
        assert!(asked.contains("\"\"\"\nBump the deps.\n\"\"\""));
        assert!(asked.contains("\"done_when\"") && asked.contains("\"evidence\""));
        assert!(asked.contains(".moor/artifacts/plan.json"));
        assert!(is_draft_prompt(&asked) && !is_draft_prompt("Bump the deps."));
    }
}
