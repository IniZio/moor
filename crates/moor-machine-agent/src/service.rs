//! The user service that keeps `run` going: a systemd user unit on Linux, a launchd agent on
//! macOS. Both belong to the person, so installing one needs no root.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The systemd unit's name.
pub const UNIT: &str = "moor-machine-agent.service";
/// The launchd agent's label.
pub const LABEL: &str = "dev.moor.machine-agent";
/// Names the directory the service file goes in, in place of the platform's own.
pub const DIR_ENV: &str = "MOOR_AGENT_SERVICE_DIR";

/// What the service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The agent's executable.
    pub exe: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its environment, beside what the service manager gives every service.
    pub env: Vec<(String, String)>,
    /// Where launchd writes its stderr; systemd keeps it in the journal.
    pub log: PathBuf,
}

/// `word` as one quoted systemd word: `%` is a specifier there, so it is doubled.
fn quoted(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// The systemd user unit for `service`.
#[must_use]
pub fn systemd(service: &Service) -> String {
    let mut exec = quoted(&service.exe.to_string_lossy());
    for arg in &service.args {
        exec.push(' ');
        exec.push_str(&quoted(arg));
    }
    let mut env = String::new();
    for (k, v) in &service.env {
        let _ = writeln!(env, "Environment={}", quoted(&format!("{k}={v}")));
    }
    format!(
        "[Unit]
Description=moor machine agent: keeps this machine linked to its moor server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart={exec}
{env}Restart=always
RestartSec=5

[Install]
WantedBy=default.target
"
    )
}

/// `text` safe inside an XML element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd agent's property list for `service`.
#[must_use]
pub fn launchd(service: &Service) -> String {
    let mut args = String::new();
    for arg in std::iter::once(service.exe.to_string_lossy().into_owned())
        .chain(service.args.iter().cloned())
    {
        let _ = writeln!(args, "    <string>{}</string>", xml(&arg));
    }
    let mut env = String::new();
    for (k, v) in &service.env {
        let _ = writeln!(
            env,
            "    <key>{}</key>\n    <string>{}</string>",
            xml(k),
            xml(v)
        );
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml(&service.log.to_string_lossy())
    )
}

/// The person's service manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Manager {
    /// `systemctl --user`, with the unit in `dir`.
    Systemd {
        /// Where the unit file goes.
        dir: PathBuf,
    },
    /// launchd's per-user domain, with the property list in `dir`.
    Launchd {
        /// Where the property list goes.
        dir: PathBuf,
        /// The person's uid, which names the domain.
        uid: u32,
    },
}

impl Manager {
    /// This platform's manager for the person in `home`, its directory overridden by
    /// [`DIR_ENV`].
    #[must_use]
    pub fn here(home: &Path) -> Self {
        let over = std::env::var_os(DIR_ENV).map(PathBuf::from);
        if cfg!(target_os = "macos") {
            Self::Launchd {
                dir: over.unwrap_or_else(|| home.join("Library/LaunchAgents")),
                uid: rustix::process::getuid().as_raw(),
            }
        } else {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from);
            Self::Systemd {
                dir: over.unwrap_or_else(|| config.join("systemd/user")),
            }
        }
    }

    /// The service file's path.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        match self {
            Self::Systemd { dir } => dir.join(UNIT),
            Self::Launchd { dir, .. } => dir.join(format!("{LABEL}.plist")),
        }
    }

    /// `service` as this manager's file.
    #[must_use]
    pub fn render(&self, service: &Service) -> String {
        match self {
            Self::Systemd { .. } => systemd(service),
            Self::Launchd { .. } => launchd(service),
        }
    }

    /// Whether the file on disk differs from `service`'s.
    #[must_use]
    pub fn differs(&self, service: &Service) -> bool {
        fs::read_to_string(self.file()).ok() != Some(self.render(service))
    }

    /// Writes `service`'s file, makes it start at login, and starts it: anew when the file
    /// changed, else only when it is not running.
    ///
    /// # Errors
    /// The file cannot be written or the manager refuses.
    pub fn install(&self, service: &Service) -> Result<(), String> {
        let changed = self.differs(service);
        let file = self.file();
        if changed {
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
            }
            fs::write(&file, self.render(service))
                .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        }
        match self {
            Self::Systemd { .. } => {
                systemctl(&["daemon-reload"])?;
                systemctl(&["enable", UNIT])?;
                systemctl(&[if changed { "restart" } else { "start" }, UNIT])
            }
            Self::Launchd { uid, .. } => {
                let target = format!("gui/{uid}/{LABEL}");
                if changed {
                    let _ = run("launchctl", &["bootout", &target]);
                }
                if run("launchctl", &["print", &target]).is_ok() {
                    return Ok(());
                }
                run(
                    "launchctl",
                    &["bootstrap", &format!("gui/{uid}"), &file.to_string_lossy()],
                )
            }
        }
    }

    /// Stops the service and removes its file; whether there was one.
    ///
    /// # Errors
    /// The manager refuses or the file cannot be removed.
    pub fn remove(&self) -> Result<bool, String> {
        let file = self.file();
        if !file.exists() {
            return Ok(false);
        }
        match self {
            Self::Systemd { .. } => systemctl(&["disable", "--now", UNIT])?,
            Self::Launchd { uid, .. } => {
                let _ = run("launchctl", &["bootout", &format!("gui/{uid}/{LABEL}")]);
            }
        }
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove {}: {e}", file.display())),
        }
        if matches!(self, Self::Systemd { .. }) {
            systemctl(&["daemon-reload"])?;
        }
        Ok(true)
    }

    /// Where the service's log is read.
    #[must_use]
    pub fn log_hint(&self, service_log: &Path) -> String {
        match self {
            Self::Systemd { .. } => format!("journalctl --user -u {UNIT}"),
            Self::Launchd { .. } => service_log.display().to_string(),
        }
    }
}

/// `systemctl --user args`.
fn systemctl(args: &[&str]) -> Result<(), String> {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    run("systemctl", &all)
}

/// Runs `program args`, failing with what it said when it fails.
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "{program} {} failed: {}",
        args.join(" "),
        said.trim()
    ))
}
