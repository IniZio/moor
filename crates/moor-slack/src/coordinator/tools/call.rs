//! One tool call, parsed from its name and arguments.

use serde_json::Value;

use super::answer::number;

/// One tool call, parsed from its name and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    /// `list_boxes`.
    ListBoxes,
    /// `start_box`.
    StartBox {
        /// What the Box should do.
        goal: String,
        /// The repo as given, when given.
        repo: Option<String>,
        /// The branch or ref to start from, when given.
        base: Option<String>,
    },
    /// `steer_box`.
    SteerBox {
        /// The Box's name as given.
        box_name: String,
        /// What to send.
        text: String,
    },
    /// `close_box`.
    CloseBox {
        /// The Box's name as given.
        box_name: String,
        /// Discard work the Box has not pushed.
        discard: bool,
    },
    /// `answer_box`.
    AnswerBox {
        /// The Box's name as given.
        box_name: String,
        /// The id of the prompt to answer.
        prompt: i64,
        /// The option to pick, numbered from 1.
        choice: u32,
    },
    /// `show_box`.
    ShowBox {
        /// The Box's name as given.
        box_name: String,
    },
    /// `interrupt_box`.
    InterruptBox {
        /// The Box's name as given.
        box_name: String,
    },
    /// `sleep_box`.
    SleepBox {
        /// The Box's name as given.
        box_name: String,
    },
    /// `wake_box`.
    WakeBox {
        /// The Box's name as given.
        box_name: String,
    },
    /// `restart_agent`.
    RestartAgent {
        /// The Box's name as given.
        box_name: String,
    },
}

impl Call {
    /// The call `name` with `args`, or why it is not one.
    pub(crate) fn parse(name: &str, args: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{name} needs a non-empty `{key}`."))
        };
        let boxed = || text("box");
        match name {
            "list_boxes" => Ok(Self::ListBoxes),
            "start_box" => Ok(Self::StartBox {
                goal: text("goal")?,
                repo: text("repo").ok(),
                base: text("base").ok(),
            }),
            "steer_box" => Ok(Self::SteerBox {
                box_name: boxed()?,
                text: text("text")?,
            }),
            "close_box" => Ok(Self::CloseBox {
                box_name: boxed()?,
                discard: args
                    .get("discard")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }),
            "answer_box" => Ok(Self::AnswerBox {
                box_name: boxed()?,
                prompt: number(name, args, "prompt")?,
                choice: number(name, args, "choice")?,
            }),
            "show_box" => Ok(Self::ShowBox { box_name: boxed()? }),
            "interrupt_box" => Ok(Self::InterruptBox { box_name: boxed()? }),
            "sleep_box" => Ok(Self::SleepBox { box_name: boxed()? }),
            "wake_box" => Ok(Self::WakeBox { box_name: boxed()? }),
            "restart_agent" => Ok(Self::RestartAgent { box_name: boxed()? }),
            other => Err(format!("There is no tool `{other}`.")),
        }
    }
}
