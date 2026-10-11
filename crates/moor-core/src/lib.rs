//! Core domain types and the request/response wire contract for moor.
#![forbid(unsafe_code)]

pub mod domain;
pub mod name;
pub mod request;
pub mod response;
pub mod seam;
pub mod secret;

pub use name::{BoxHandle, BoxPath, NameError, ProviderId, RunId, Sha, TaskId, TaskName};
pub use request::{
    CloseReq, CpReq, EnvReq, GateAction, GateReq, RemoveReq, ReportReq, Request, RunReq, ShowReq,
    StartReq, SteerReq, TaskReq, WaitFor, WaitReq,
};
pub use response::{ErrorKind, ErrorView, ExitClass, Response, Warning, SCHEMA};
pub use secret::{Digest, Secret};
