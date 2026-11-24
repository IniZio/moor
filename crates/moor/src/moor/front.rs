//! The requests only a front sends, for a Box or for every user, which assert no identity.

use moor_core::request::{BoxToolReq, Grant, Pins};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{ErrorKind, Request, Response};

use super::Moor;
use crate::error::CommandError;
use crate::ops;
use crate::ops::box_tool::Plan;
use crate::vault::Vault;

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

    /// Allows the `box-tool` `req` from `caller`, asserting `asserted`: what the request may
    /// see, whom it acts as, and the `exec` or `start` it runs.
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
        };
        Ok((grant, admitted.who, request))
    }
}
