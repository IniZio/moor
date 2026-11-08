//! An hc front for moor. Each open Box is an hc machine on the person's computer, reached by
//! `moor-hc exec`, which pipes hc's bytes to the Box's own hc through the moor server.

pub mod boxes;
pub mod error;
pub mod exec;
pub mod hc;
pub mod plan;
pub mod service;
pub mod sync;
pub mod via;
