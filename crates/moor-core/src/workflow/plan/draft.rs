//! What the agent is told when it writes a plan instead of following one: a short Box that drafts
//! a workflow's plan, and a one-off that proposes its plan and waits for approval first.

use super::PlanDoc;
use crate::gate::GatesDoc;
use crate::reading::HandIn;

/// The version of what a reader is told: [`draft_prompt`]'s instructions. Bump it with every change
/// to them, so a reading says which instructions it was read under.
pub const READER_INSTRUCTIONS: u32 = 2;

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
        "Hand in gates when the prompt has each run pass named gates in order (it says moor \
         holds the run open until its gates pass, or asks for report_step for a gate). Name \
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
         gates pass, such as nothing to do. Each gate and outcome you hand in is one entry of \
         `gates` or `outcomes` in this schema, without made_from:\n{schema}"
    )
}

/// How to hand in a reading through moor's `hand_in_reading` tool, with its quoting rules.
fn how_to_hand_in() -> String {
    let schema = schemars::schema_for!(HandIn);
    let schema = serde_json::to_string(&schema).unwrap_or_default();
    format!(
        "Hand in a reading with the hand_in_reading tool of the moor MCP server. It takes \
         phrases of the prompt and the effects moor holds the agent to. Quote the prompt's own \
         words in `exact`, copied, never reworded. When those words occur more than once in \
         the prompt, add a `prefix` (the words just before) or a `suffix` (the words just \
         after) that picks one. Make one phrase per clause moor can hold, and never let two \
         phrases share words. Mark a phrase held only when it names at least one effect by \
         its label; mark every other phrase you quote asked. A clause with a part moor can hold \
         and a part it can only ask is two phrases. Every effect must be named by a held \
         phrase. An allow effect must be a line the config's proxy.allow already lists, copied \
         under the same rule: name no other. When moor refuses a hand-in it says which quote \
         or effect is wrong; fix that one and hand in again. Three refusals end the reading. \
         Once moor answers accepted, end your turn. The tool's input matches this \
         schema:\n{schema}"
    )
}

/// How a draft prompt opens, so a front can tell a drafting Box from a person's own.
const DRAFT_HEAD: &str = "Draft a plan for this workflow prompt.";

/// Whether `text` is a prompt `draft_prompt` made.
#[must_use]
pub fn is_draft_prompt(text: &str) -> bool {
    text.starts_with(DRAFT_HEAD)
}

/// What a short Box that reads the workflow prompt `prompt` is asked, with `config` its runs get.
///
/// It hands in a reading when the prompt names gates or anything moor can hold, and leaves a
/// step plan as a file otherwise.
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
         prompt needs, then give moor its reading in one of two ways. Each run starts a new Box \
         on its own branch, so name no branch, Box or folder of this one.\n\nWhen the prompt \
         names gates, or anything moor can hold, such as a line of the config's proxy.allow, \
         hand in a reading.\n\n{}\n\n{}\n\nWhen the prompt instead calls for steps a run \
         walks, write a plan as .moor/artifacts/{PLAN_FILE} and nothing else, then end your \
         turn.\n\n{}",
        prompt.trim(),
        how_to_hand_in(),
        how_to_gate(),
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
        let asked = draft_prompt("Bump the deps.", None);
        assert!(asked.contains("\"\"\"\nBump the deps.\n\"\"\""));
        assert!(asked.contains("\"done_when\"") && asked.contains("\"evidence\""));
        assert!(asked.contains(".moor/artifacts/plan.json"));
        assert!(is_draft_prompt(&asked) && !is_draft_prompt("Bump the deps."));
    }

    #[test]
    fn a_draft_prompt_asks_for_a_hand_in_by_the_prompts_own_words() {
        let asked = draft_prompt("Post one comment. Never merge.", None);
        assert!(asked.contains("hand_in_reading") && asked.contains("\"exact\""));
        for rule in [
            "prompt's own",
            "prefix",
            "one phrase per clause",
            "proxy.allow",
        ] {
            assert!(asked.contains(rule), "the instructions state {rule}");
        }
    }
}
