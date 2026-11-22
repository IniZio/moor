//! What the setup script and dotfiles delivery did, as `start` and `show` report them.

/// Outcome of the setup script.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct SetupView {
    /// Whether setup ran.
    pub ran: bool,
    /// Whether it succeeded.
    pub ok: bool,
    /// Last output line.
    pub line: Option<String>,
    /// Duration in milliseconds.
    pub ms: Option<u64>,
    /// Last lines of the setup log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
}

/// Outcome of dotfiles delivery.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct DotfilesView {
    /// Whether dotfiles ran.
    pub ran: bool,
    /// Whether it succeeded.
    pub ok: bool,
    /// Duration in milliseconds.
    pub ms: Option<u64>,
}
