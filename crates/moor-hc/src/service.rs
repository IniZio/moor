//! The user service that keeps `moor-hc sync` running: a systemd user unit on Linux, a launchd
//! agent on macOS. moor-hc prints it with where to put it; it installs nothing itself.

use std::fmt::Write as _;

/// The launchd label and the systemd unit's name.
pub const NAME: &str = "moor-hc";

/// What the service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The `moor-hc` binary, by absolute path.
    pub exe: String,
    /// The `sync` arguments after the binary.
    pub args: Vec<String>,
    /// The `PATH` the service runs with, so it finds `hc`.
    pub path: String,
}

impl Service {
    /// The service file for `os`, as `std::env::consts::OS` names it, then how to start it.
    #[must_use]
    pub fn render(&self, os: &str) -> (String, String) {
        if os == "macos" {
            (self.plist(), format!("Save it as ~/Library/LaunchAgents/{NAME}.plist, then run:\n  launchctl load -w ~/Library/LaunchAgents/{NAME}.plist"))
        } else {
            (self.unit(), format!("Save it as ~/.config/systemd/user/{NAME}.service, then run:\n  systemctl --user daemon-reload && systemctl --user enable --now {NAME}"))
        }
    }

    /// The systemd user unit. `KillMode=process` because the first `hc` call may start hc's
    /// core, which must outlive a restart of sync.
    fn unit(&self) -> String {
        let words: Vec<String> = std::iter::once(&self.exe)
            .chain(&self.args)
            .map(|w| systemd_quote(w))
            .collect();
        format!(
            "[Unit]\nDescription=moor-hc: one hc machine per up moor Box\n\n[Service]\nExecStart={}\nEnvironment={}\nRestart=always\nRestartSec=10\nKillMode=process\n\n[Install]\nWantedBy=default.target\n",
            words.join(" "),
            systemd_quote(&format!("PATH={}", self.path)),
        )
    }

    /// The launchd agent. `AbandonProcessGroup` keeps an hc core sync started running.
    fn plist(&self) -> String {
        let args =
            std::iter::once(&self.exe)
                .chain(&self.args)
                .fold(String::new(), |mut acc, a| {
                    let _ = writeln!(acc, "    <string>{}</string>", xml_escape(a));
                    acc
                });
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{NAME}</string>\n  <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n  <key>AbandonProcessGroup</key>\n  <true/>\n</dict>\n</plist>\n",
            xml_escape(&self.path)
        )
    }
}

/// `s` as one systemd command-line word.
fn systemd_quote(s: &str) -> String {
    let escaped = s
        .replace('\\', r"\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

/// `s` as XML text.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests;
