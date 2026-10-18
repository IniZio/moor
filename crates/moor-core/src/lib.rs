//! Core domain types and the request/response wire contract for moor.
#![forbid(unsafe_code)]

pub mod activity;
pub mod attention;
pub mod box_key;
pub mod boxscript;
pub mod config;
pub mod controller;
pub mod decide;
pub mod domain;
pub mod exec_env;
pub mod graphql_scope;
pub mod name;
pub mod request;
pub mod response;
pub mod seam;
pub mod secret;
pub mod view;

pub use name::{
    BoxHandle, BoxId, BoxName, BoxPath, NameError, ProviderId, RunId, SessionId, Sha, TaskId,
    TaskName,
};
pub use request::{
    CloseReq, CpReq, EnvReq, GateReq, RemoveReq, ReportReq, Request, RunReq, ShowReq, StartReq,
    SteerReq, TaskReq, WaitFor, WaitReq,
};
pub use response::{ErrorKind, ErrorView, ExitClass, Response, Warning, SCHEMA};
pub use secret::{Digest, Secret};
