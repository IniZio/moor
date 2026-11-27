//! One run read by its id alone, which any signed-in person may do.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowRunReadReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::WorkflowRunReadValue;
use serde_json::{json, Value};

use super::run::run_view;
use super::{not_found, value_of, Failure};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::Session;
use crate::Gate;

/// Answers run `id` of any workflow, with `yours` saying whether the person owns it.
pub(crate) fn run_by_id(stream: &mut TcpStream, gate: &Gate, session: &Session, id: i64) {
    let read = value_of::<WorkflowRunReadValue>(
        gate,
        session,
        ControllerRequest::WorkflowRunRead(WorkflowRunReadReq {
            identity: None,
            run: id,
        }),
    );
    match read {
        Ok(read) => {
            let mut view = run_view(&read.workflow, &read.run);
            if let Value::Object(map) = &mut view {
                map.insert("yours".to_owned(), json!(read.yours));
                map.insert("outputs".to_owned(), json!(read.outputs));
                map.insert("contract".to_owned(), json!(read.contract));
            }
            json(stream, "200 OK", &view);
        }
        Err(Failure::Reply(reply)) if reply.class == ExitClass::Refused => {
            json(stream, "404 Not Found", &json!({"error": "no such run"}));
        }
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Missing) => not_found(stream),
        Err(Failure::Other(why)) => failed(stream, "run read", &why),
    }
}
