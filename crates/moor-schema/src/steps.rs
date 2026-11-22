//! The store's migrations. A new step adds one `mod` line and one `step` line, so two branches
//! that each add one merge line by line (see `.gitattributes`).

use moor_migrate::Step;

use crate::step;

mod agent_reads;
mod baseline;
mod box_base_ref;
mod box_config;
mod box_deaths;
mod box_gates;
mod box_hands;
mod box_inbox;
mod box_state;
mod checkpoint_base;
mod checkpoint_depth;
mod checkpoint_stale;
mod drop_box_deaths;
mod drop_checkpoints;
mod drop_panes;
mod drop_workflow_breaker;
mod effect_backfill;
mod egress_token;
mod forward_port;
mod hands_ended;
mod held_prompt;
mod inbox_prompts;
mod insertion_order;
mod launch_facts;
mod launch_fence;
mod link_provides;
mod link_runner;
mod machine_boxes;
mod machine_pairings;
mod node_settings;
mod pause_publish;
mod proxy_key;
mod proxy_state;
mod repo_config;
mod resume_memory;
mod said_at;
mod session_archive;
mod shared_machines;
mod suspend_check;
mod user_inbox;
pub(crate) mod vm_inc;
mod wanted_effect;
mod warm_starts;
mod workflow_also;
mod workflow_changes;
mod workflow_gates;
mod workflow_on_pull;
mod workflow_optional_cron;
mod workflow_plans;
mod workflow_repo_set;
mod workflow_run_pull;
mod workflow_run_trigger;
mod workflow_run_verdict;
mod workflows;

/// The store's migrations, oldest first. Never edit an applied one; add a new step.
///
/// The baseline squashed the SQL migrations 1 to 44, so a file at 44 is adopted.
pub const STEPS: &[Step] = &[
    step("V45__baseline", baseline::write),
    step("V46__insertion_order", insertion_order::write),
    step("V47__proxy_key", proxy_key::write),
    step("V48__workflows", workflows::write),
    step("V49__workflow_changes", workflow_changes::write),
    step("V50__resume_memory", resume_memory::write),
    step("V51__proxy_state", proxy_state::write),
    step("V52__egress_token", egress_token::write),
    step("V53__forward_port", forward_port::write),
    step("V54__suspend_check", suspend_check::write),
    step("V55__held_prompt", held_prompt::write),
    step("V56__link_runner", link_runner::write),
    step("V58__warm_starts", warm_starts::write),
    step("V59__pause_publish", pause_publish::write),
    step("V60__machine_boxes", machine_boxes::write),
    step("V61__checkpoint_base", checkpoint_base::write),
    step("V62__checkpoint_depth", checkpoint_depth::write),
    step("V63__checkpoint_stale", checkpoint_stale::write),
    step("V64__agent_reads", agent_reads::write),
    step("V65__drop_panes", drop_panes::write),
    step("V66__said_at", said_at::write),
    step("V67__box_hands", box_hands::write),
    step("V68__box_state", box_state::write),
    step("V69__session_archive", session_archive::write),
    step("V70__hands_ended", hands_ended::write),
    step("V71__vm_inc", vm_inc::write),
    step("V72__box_inbox", box_inbox::write),
    step("V73__inbox_prompts", inbox_prompts::write),
    step("V74__launch_fence", launch_fence::write),
    step("V75__wanted_effect", wanted_effect::write),
    step("V76__effect_backfill", effect_backfill::write),
    step("V77__launch_facts", launch_facts::write),
    step("V78__workflow_also", workflow_also::write),
    step("V79__drop_checkpoints", drop_checkpoints::write),
    step("V80__shared_machines", shared_machines::write),
    step("V81__repo_config", repo_config::write),
    step("V82__box_config", box_config::write),
    step("V83__link_provides", link_provides::write),
    step("V84__node_settings", node_settings::write),
    step("V85__workflow_plans", workflow_plans::write),
    step("V86__machine_pairings", machine_pairings::write),
    step("V87__workflow_run_trigger", workflow_run_trigger::write),
    step("V88__box_deaths", box_deaths::write),
    step("V89__drop_box_deaths", drop_box_deaths::write),
    step("V90__user_inbox", user_inbox::write),
    step("V91__workflow_repo_set", workflow_repo_set::write),
    step("V92__box_base_ref", box_base_ref::write),
    step("V93__workflow_run_pull", workflow_run_pull::write),
    step("V94__workflow_on_pull", workflow_on_pull::write),
    step("V95__drop_workflow_breaker", drop_workflow_breaker::write),
    step("V96__workflow_optional_cron", workflow_optional_cron::write),
    step("V97__box_gates", box_gates::write),
    step("V98__workflow_gates", workflow_gates::write),
    step("V99__workflow_run_verdict", workflow_run_verdict::write),
];
