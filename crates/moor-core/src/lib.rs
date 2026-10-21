//! Core domain types and the request/response wire contract for moor.
#![forbid(unsafe_code)]

pub mod activity;
pub mod artifact;
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
    BoxHandle, BoxId, BoxName, BoxPath, DirectHost, NameError, ProviderId, RunId, SessionId, Sha,
};
pub use request::{
    ArtifactReadReq, ArtifactsReq, BoxReq, BoxTarget, CloseReq, CpReq, EnvReq, GateReq, Request,
    RunReq, ShowReq, StartReq, SteerReq, WaitEventsReq, WaitFor, WaitMode, WaitReq,
};
pub use response::{ErrorKind, ErrorView, ExitClass, Response, Warning, SCHEMA};
pub use secret::{Digest, Secret};
