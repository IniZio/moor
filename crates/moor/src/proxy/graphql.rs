//! Loopback filter that nono chains GitHub API traffic through so each
//! GraphQL request is judged against the Task repo before it leaves.

pub(crate) mod http;
pub(crate) mod lookup;
pub(crate) mod upstream;
