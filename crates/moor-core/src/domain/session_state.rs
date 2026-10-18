//! The Session state machine: every state change is `step()`, and every other pair is an error.

use super::Illegal;

/// Where a Session is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The agent is launching.
    Starting,
    /// The agent is working.
    Working,
    /// The agent is quiet.
    Idle,
    /// The Session is over; terminal.
    Ended,
}

impl SessionState {
    /// Every state, in declaration order.
    pub const ALL: [Self; 4] = [Self::Starting, Self::Working, Self::Idle, Self::Ended];

    /// The `snake_case` name, identical to the serde form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Ended => "ended",
        }
    }

    /// Parses a `snake_case` state name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == s)
    }

    /// The state of a new Session; only `Open` creates one.
    ///
    /// # Errors
    /// Returns [`Illegal`] with `from` = `none` for any input but `Open`.
    pub fn initial(input: &SessionInput) -> Result<Self, Illegal> {
        match input {
            SessionInput::Open => Ok(Self::Starting),
            other => Err(Illegal {
                from: "none".to_owned(),
                input: other.name().to_owned(),
            }),
        }
    }

    /// The state after `input`. A sleeping Box leaves its Sessions untouched, so no input
    /// ends a Session because of sleep.
    ///
    /// # Errors
    /// Returns [`Illegal`] for any pair outside the transition table.
    pub fn step(self, input: &SessionInput) -> Result<Self, Illegal> {
        use SessionInput as I;
        let to = match (self, input) {
            (Self::Starting, I::Started) | (Self::Idle, I::AgentBusy | I::RunStarted) => {
                Self::Working
            }
            (Self::Working, I::AgentQuiet) => Self::Idle,
            (Self::Starting | Self::Working | Self::Idle, I::End) => Self::Ended,
            _ => {
                return Err(Illegal {
                    from: self.as_str().to_owned(),
                    input: input.name().to_owned(),
                });
            }
        };
        Ok(to)
    }
}

/// A fact that may move a Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionInput {
    /// A new Session is created.
    Open,
    /// The agent finished launching.
    Started,
    /// The agent went quiet.
    AgentQuiet,
    /// The agent became busy.
    AgentBusy,
    /// A Run started.
    RunStarted,
    /// The Session ends.
    End,
}

impl SessionInput {
    /// The `snake_case` variant name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Started => "started",
            Self::AgentQuiet => "agent_quiet",
            Self::AgentBusy => "agent_busy",
            Self::RunStarted => "run_started",
            Self::End => "end",
        }
    }
}

#[cfg(test)]
mod tests;
