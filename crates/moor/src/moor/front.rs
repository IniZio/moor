//! The requests only a front sends, for a Box or for every user, which assert no identity.

use moor_core::request::{BoxToolReq, Grant, Pins};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{ErrorKind, Request, Response};

use super::Moor;
use crate::error::CommandError;
use crate::ops;
use crate::ops::box_tool::{runs, Plan};
use crate::vault::Vault;
use moor_core::request::BoxRunId;
use moor_core::request::{BoxCall, BoxToolReq as ToolReq};
use moor_core::user::UserId;
use moor_core::view::BoxRunView;
use moor_core::ExecReq;
use std::time::Duration;

impl Moor {
    /// Run `webhook-claim` or `webhook-report` for `caller`, which must be a front whose config
    /// sets `webhooks: true`; it acts for every user, so it asserts no identity.
    pub(crate) fn call_webhook_outbox(
        &self,
        caller: &Result<Caller, String>,
        vault: &Vault,
        request: &Request,
    ) -> Response {
        let name = request.cmd();
        let run = || {
            ops::webhook_outbox::permit(|ns| self.cx.env().front_webhooks(ns), caller)?;
            let store = self.cx.store()?;
            match request {
                Request::WebhookClaim(req) => ops::webhook_outbox::claim(store, vault, req),
                Request::WebhookReport(req) => ops::webhook_outbox::report(store, req),
                _ => Err(CommandError::new(
                    ErrorKind::Internal,
                    "not a webhook request",
                )),
            }
        };
        match run() {
            Ok(value) => Response::ok(name, Some(value)),
            Err(e) => e.into_response(name, vec![]),
        }
    }

    /// Run `step-report` for `caller`, which must be a front whose config sets
    /// `step_reports: true`. The front names the Box from its transport and asserts no identity.
    pub(crate) fn call_step_report(
        &self,
        caller: &Result<Caller, String>,
        request: &Request,
    ) -> Response {
        let name = request.cmd();
        let run = || {
            match caller {
                Ok(Caller::Front(ns)) if self.cx.env().front_step_reports(ns) => {}
                _ => {
                    return Err(CommandError::new(
                        ErrorKind::Refused,
                        ops::authz::STEP_REPORT_FRONT_ONLY,
                    ))
                }
            }
            let Request::StepReport(req) = request else {
                return Err(CommandError::new(
                    ErrorKind::Internal,
                    "not a step-report request",
                ));
            };
            ops::step_report(&self.cx, req)
        };
        match run() {
            Ok(value) => Response::ok(name, Some(value)),
            Err(e) => e.into_response(name, vec![]),
        }
    }

    /// How long a place's `exec_in_box` or `exec_wait` waits before it answers.
    pub(crate) const BOX_RUN_WINDOW: Duration = runs::WINDOW;

    /// Allows the `box-tool` `req` from `caller`, asserting `asserted`: what the request may
    /// see, whom it acts as, and what it runs: an `exec` that runs on as a place's run, a
    /// `start` or `close`, or, for a wait on a run, the `box-tool` itself.
    ///
    /// # Errors
    /// The refusal or store failure the request answers with instead.
    pub(crate) fn admit_box_tool(
        &self,
        caller: &Result<Caller, String>,
        asserted: Option<&Identity>,
        req: &BoxToolReq,
    ) -> Result<(Grant, Identity, Request), CommandError> {
        let admitted = ops::box_tool::admit(&self.cx, caller, asserted, req)?;
        let grant = Grant {
            viewer: Viewer::User(admitted.user),
            pins: Pins::new(),
        };
        let request = match admitted.plan {
            Plan::Exec(exec) => Request::Exec(exec),
            Plan::Start(start) => Request::Start(*start),
            Plan::Close(close) => Request::Close(close),
            Plan::Wait(run) => Request::BoxTool(ToolReq {
                from: req.from,
                call: BoxCall::Wait { run },
            }),
        };
        Ok((grant, admitted.who, request))
    }

    /// A place's close reply as its agent can act on it; see [`ops::box_tool::for_place`].
    pub(crate) fn for_place(reply: Response) -> Response {
        ops::box_tool::for_place(reply)
    }

    /// Starts `req` as a run of `user` on a thread of its own, stopping it once `stop` says so;
    /// the run's id, which [`Self::answer_box_run`] reads.
    pub(crate) fn start_box_run(
        self,
        user: UserId,
        req: ExecReq,
        stop: impl Fn() -> bool + Send + Sync + 'static,
    ) -> BoxRunId {
        let started = runs::register(&self.cx, user);
        let id = started.id;
        std::thread::spawn(move || {
            let clock = self.cx.clock_arc();
            started.run(|| clock.instant(), &stop, |pipes| self.exec(&req, pipes));
        });
        id
    }

    /// Waits up to `window` on run `id` of `user`, or until `stop` says so; its output since
    /// the last answer, and how it ended once it has.
    ///
    /// # Errors
    /// The refusal of a run that is not `user`'s, or the error of a command that never ran.
    pub(crate) fn answer_box_run(
        &self,
        user: UserId,
        id: BoxRunId,
        window: Duration,
        stop: &dyn Fn() -> bool,
    ) -> Result<BoxRunView, CommandError> {
        runs::answer(&self.cx, user, id, window, stop)
    }
}
