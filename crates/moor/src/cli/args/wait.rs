//! Arguments of the `wait` verb.

use clap::{Args, ValueEnum};
use moor_core::{BoxName, WaitFor, WaitReq};

use super::task_name;

/// States `wait --for` accepts.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum WaitForArg {
    /// Agent idle.
    Idle,
    /// Task done.
    Done,
    /// Agent needs input, or is idle.
    Input,
    /// Run finished.
    Run,
}

/// Arguments of `wait`.
#[derive(Debug, Args)]
pub(super) struct WaitArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: BoxName,
    /// State to wait for.
    #[arg(long = "for", value_enum)]
    wait_for: Option<WaitForArg>,
    /// Timeout in seconds (default 1800).
    #[arg(long, value_name = "S")]
    timeout: Option<u64>,
}

impl From<WaitArgs> for WaitReq {
    fn from(a: WaitArgs) -> Self {
        let wait_for = a.wait_for.map(|w| match w {
            WaitForArg::Idle => WaitFor::Idle,
            WaitForArg::Done => WaitFor::Done,
            WaitForArg::Input => WaitFor::Input,
            WaitForArg::Run => WaitFor::Run,
        });
        Self {
            box_name: a.task,
            wait_for,
            timeout_secs: a.timeout,
        }
    }
}
