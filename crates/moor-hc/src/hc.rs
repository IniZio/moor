//! The hc CLI calls sync makes: list, add and remove machines.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::Error;
use crate::plan::{Machine, Step};
use crate::via::Via;

/// hc's machine list and changes to it.
pub trait Hc {
    /// Every machine hc has.
    fn machines(&mut self) -> Result<Vec<Machine>, Error>;
    /// Makes one planned change.
    fn apply(&mut self, step: &Step) -> Result<(), Error>;
}

/// One row of `hc machine ls --json`.
#[derive(Debug, Deserialize)]
struct Row {
    /// hc's name for the machine.
    name: String,
    /// The ssh target, or the via command.
    target: String,
    /// Whether `target` is a via command.
    #[serde(default)]
    via: bool,
}

/// The value of `hc machine ls --json`.
#[derive(Debug, Deserialize)]
struct List {
    /// The machines.
    machines: Vec<Row>,
}

/// The `hc` binary.
#[derive(Debug, Clone)]
pub struct Cli {
    /// Its path, or a name to look up on `PATH`.
    pub bin: PathBuf,
}

impl Cli {
    /// Runs `hc <args> --json` and returns its stdout.
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        let fail = |message: String| Error::Hc {
            args: args.join(" "),
            message,
        };
        let out = Command::new(&self.bin)
            .args(args)
            .arg("--json")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| fail(format!("{}: {e}", self.bin.display())))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        let said = if out.stdout.is_empty() {
            &out.stderr
        } else {
            &out.stdout
        };
        Err(fail(String::from_utf8_lossy(said).trim().to_owned()))
    }
}

impl Hc for Cli {
    fn machines(&mut self) -> Result<Vec<Machine>, Error> {
        let list: List = serde_json::from_slice(&self.run(&["machine", "ls"])?)?;
        Ok(list
            .machines
            .into_iter()
            .map(|r| Machine {
                via: r.via.then(|| Via::parse(&r.target)).flatten(),
                name: r.name,
            })
            .collect())
    }

    fn apply(&mut self, step: &Step) -> Result<(), Error> {
        match step {
            Step::Remove { name } => self.run(&["machine", "rm", name]),
            Step::Add { name, via } => self.run(&[
                "machine",
                "add",
                "--name",
                name,
                "--temp",
                "--via",
                &via.render(),
            ]),
        }
        .map(drop)
    }
}
