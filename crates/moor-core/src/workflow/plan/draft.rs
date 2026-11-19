//! What the agent is told when it writes a plan instead of following one: a short Box that drafts
//! a workflow's plan, and a one-off that proposes its plan and waits for approval first.

use super::PlanDoc;

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

/// What a short Box that drafts a workflow's plan is asked, for the workflow prompt `prompt`.
#[must_use]
pub fn draft_prompt(prompt: &str) -> String {
    format!(
        "Draft a plan for this workflow prompt. Only read: change no file, branch or pull \
         request.\n\nThe workflow prompt:\n\"\"\"\n{}\n\"\"\"\n\nRead the repos to see what the \
         prompt needs, then write its plan. Each run starts a new Box on its own branch, so name no \
         branch, Box or folder of this one.\n\n{}\n\nWrite the plan as .moor/artifacts/{PLAN_FILE} \
         and nothing else, then end your turn.",
        prompt.trim(),
        how_to_write()
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
        let asked = draft_prompt("Bump the deps.");
        assert!(asked.contains("\"\"\"\nBump the deps.\n\"\"\""));
        assert!(asked.contains("\"done_when\"") && asked.contains("\"evidence\""));
        assert!(asked.contains(".moor/artifacts/plan.json"));
    }
}
