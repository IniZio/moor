//! What `moor box-term` runs where.

/// What `box-term` runs where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxTerm {
    /// The runner that holds the machine's link.
    pub runner: String,
    /// The machine's id.
    pub machine_id: i64,
    /// The Box.
    pub slug: String,
    /// The place in it.
    pub place: moor_core::seam::driver::Place,
    /// The script.
    pub script: String,
}
