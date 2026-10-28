//! herdr as this front drives it: workspaces, their tokens, and the agent dot of a pane.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

use crate::error::Error;
use crate::picker::Bench;

/// The metadata source and agent-report source this front writes as.
pub const SOURCE: &str = "moor";

/// A herdr workspace as `workspace list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Space {
    /// herdr's id, such as `w2`.
    #[serde(rename = "workspace_id")]
    pub id: String,
    /// The label in the sidebar.
    pub label: String,
    /// Metadata tokens reported on it; herdr forgets them when its server restarts.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// The rolled-up agent state of its panes.
    #[serde(default)]
    pub agent_status: String,
}

/// A workspace just made, and its first pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    /// The workspace's id.
    pub workspace: String,
    /// Its root pane's id.
    pub pane: String,
}

/// The herdr calls this front makes. [`Cli`] runs the herdr binary; tests use a fake.
pub trait Herdr {
    /// Every workspace.
    fn spaces(&mut self) -> Result<Vec<Space>, Error>;
    /// Makes a workspace labelled `label` without moving focus.
    fn create(&mut self, label: &str) -> Result<Created, Error>;
    /// Runs `command` in `pane`'s shell.
    fn run(&mut self, pane: &str, command: &str) -> Result<(), Error>;
    /// Sets metadata tokens on `workspace`; an empty value clears that token.
    fn tag(&mut self, workspace: &str, tokens: &[(&str, &str)]) -> Result<(), Error>;
    /// The first pane of `workspace`.
    fn first_pane(&mut self, workspace: &str) -> Result<Option<String>, Error>;
    /// Sets the agent dot of `pane`.
    fn report_agent(&mut self, pane: &str, state: &str) -> Result<(), Error>;
}

/// The herdr CLI: `$HERDR_BIN_PATH` inside a plugin, else `herdr` on `PATH`.
#[derive(Debug, Clone)]
pub struct Cli {
    /// The binary to run.
    bin: OsString,
}

impl Cli {
    /// The herdr this process runs under.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bin: std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into()),
        }
    }

    /// Runs one call and returns its `result`.
    fn call(&self, args: &[&str]) -> Result<Value, Error> {
        let fail = |message: String| Error::Herdr {
            args: args.join(" "),
            message,
        };
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| fail(e.to_string()))?;
        if !out.status.success() {
            return Err(fail(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        if out.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        let mut reply: Value =
            serde_json::from_slice(&out.stdout).map_err(|e| fail(e.to_string()))?;
        Ok(reply.get_mut("result").map_or(Value::Null, Value::take))
    }

    /// Opens moor's picker as a popup, for new work beside `workspace`.
    pub fn open_picker(&self, workspace: &str) -> Result<(), Error> {
        let env = format!("MOOR_HERDR_WORKSPACE={workspace}");
        let args = [
            "plugin",
            "pane",
            "open",
            "--plugin",
            SOURCE,
            "--entrypoint",
            "new",
            "--placement",
            "popup",
            "--width",
            "70",
            "--height",
            "18",
            "--env",
            &env,
        ];
        self.call(&args).map(drop)
    }

    /// Closes `workspace` with its panes.
    pub fn close_space(&self, workspace: &str) -> Result<(), Error> {
        self.call(&["workspace", "close", workspace]).map(drop)
    }

    /// The string at `path` in `value`.
    fn text(value: &Value, path: &str) -> Result<String, Error> {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Herdr {
                args: path.to_owned(),
                message: "missing from herdr's answer".to_owned(),
            })
    }
}

impl Herdr for Cli {
    fn spaces(&mut self) -> Result<Vec<Space>, Error> {
        let mut reply = self.call(&["workspace", "list"])?;
        let list = reply.get_mut("workspaces").map_or(Value::Null, Value::take);
        serde_json::from_value(list).map_err(|e| Error::Herdr {
            args: "workspace list".to_owned(),
            message: e.to_string(),
        })
    }

    fn create(&mut self, label: &str) -> Result<Created, Error> {
        let args = [
            "workspace",
            "create",
            "--cwd",
            "/",
            "--label",
            label,
            "--no-focus",
        ];
        let reply = self.call(&args)?;
        Ok(Created {
            workspace: Self::text(&reply, "/workspace/workspace_id")?,
            pane: Self::text(&reply, "/root_pane/pane_id")?,
        })
    }

    fn run(&mut self, pane: &str, command: &str) -> Result<(), Error> {
        self.call(&["pane", "run", pane, command]).map(drop)
    }

    fn tag(&mut self, workspace: &str, tokens: &[(&str, &str)]) -> Result<(), Error> {
        let pairs: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let mut args = vec![
            "workspace",
            "report-metadata",
            workspace,
            "--source",
            SOURCE,
        ];
        for pair in &pairs {
            args.extend(["--token", pair.as_str()]);
        }
        self.call(&args).map(drop)
    }

    fn first_pane(&mut self, workspace: &str) -> Result<Option<String>, Error> {
        let reply = self.call(&["pane", "list", "--workspace", workspace])?;
        Ok(reply
            .pointer("/panes/0/pane_id")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    fn report_agent(&mut self, pane: &str, state: &str) -> Result<(), Error> {
        let args = [
            "pane",
            "report-agent",
            pane,
            "--source",
            SOURCE,
            "--agent",
            "claude",
            "--state",
            state,
        ];
        self.call(&args).map(drop)
    }
}

impl Bench for Cli {
    fn tab(&mut self, workspace: &str) -> Result<(), Error> {
        self.call(&["tab", "create", "--workspace", workspace, "--focus"])
            .map(drop)
    }

    fn worktree(&mut self, workspace: &str, name: &str) -> Result<(), Error> {
        let args = [
            "worktree",
            "create",
            "--workspace",
            workspace,
            "--branch",
            name,
            "--focus",
        ];
        self.call(&args).map(drop)
    }

    fn focus(&mut self, workspace: &str) -> Result<(), Error> {
        self.call(&["workspace", "focus", workspace]).map(drop)
    }
}
