//! The tool a Box's owner answers a gate with: the question a gate asks before it passes.

use moor_core::name::BoxName;
use moor_core::request::{BoxTarget, Request as ControllerRequest};
use moor_core::workflow::Answer;
use moor_core::GateAnswerReq;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::Tools;
use crate::mcp::WRITE;

/// What the owner answers.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum AnswerArg {
    /// Pass the gate.
    Yes,
    /// Fail this try of the gate.
    No,
}

/// Arguments of `answer_gate`.
#[derive(Deserialize, JsonSchema)]
struct AnswerGateArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
    /// `yes` or `no`.
    answer: AnswerArg,
    /// What to tell the agent along with a no, such as what to fix; none when left out.
    note: Option<String>,
    /// The `needs.id` of the question a Box's progress showed, so a late answer never lands on a
    /// later question; left out, it answers the question the Box asks now.
    ask: Option<String>,
}

#[tool_router(router = gate_router, vis = "pub(super)")]
impl Tools {
    /// Answers the gate of one of the caller's Boxes that asks its owner.
    #[tool(
        description = "Answer yes or no to the gate of one of your Boxes that asks for your \
                       approval (its progress needs a card of kind approve with a gate). Yes \
                       passes the gate. No fails the try: the note goes to the agent, which \
                       tries again while the gate has tries left, and the Box fails when it \
                       has none. A gate that is stuck asks no answer; steer its Box with \
                       send_to_box to reopen it. Only the Box's owner can answer, and a Box \
                       with no gate that asks is refused with what it waits for. Needs the \
                       moor:write scope."
    )]
    async fn answer_gate(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<AnswerGateArgs>,
    ) -> CallToolResult {
        let request = BoxName::parse_allow_bare(&args.r#box)
            .map(|name| {
                ControllerRequest::GateAnswer(GateAnswerReq {
                    target: BoxTarget::Name(name),
                    answer: match args.answer {
                        AnswerArg::Yes => Answer::Yes,
                        AnswerArg::No => Answer::No,
                    },
                    note: args.note,
                    ask: args.ask,
                })
            })
            .map_err(|e| e.to_string());
        self.run(&context, WRITE, request, Reply::AsIs).await
    }
}
