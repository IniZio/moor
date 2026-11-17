//! The user service files `up` writes, rendered for systemd and launchd.
#![allow(clippy::unwrap_used, reason = "tests")]

use std::path::PathBuf;

use moor_machine_agent::service::{launchd, systemd, Service};

fn service() -> Service {
    Service {
        exe: PathBuf::from("/home/ada/.local/bin/moor-machine-agent"),
        args: vec![
            "run".to_owned(),
            "--agent-image".to_owned(),
            "my-image".to_owned(),
            "--work-image".to_owned(),
            "/opt/my tools/work".to_owned(),
        ],
        env: vec![
            ("PATH".to_owned(), "/usr/bin:/home/ada/100%".to_owned()),
            (
                "XDG_STATE_HOME".to_owned(),
                "/home/ada/.local/state".to_owned(),
            ),
        ],
        log: PathBuf::from("/home/ada/Library/Logs/moor-machine-agent.log"),
    }
}

#[test]
fn the_systemd_unit_runs_the_agent_again_whenever_it_stops_and_starts_at_login() {
    let unit = systemd(&service());
    assert_eq!(
        unit,
        "[Unit]
Description=moor machine agent: keeps this machine linked to its moor server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=\"/home/ada/.local/bin/moor-machine-agent\" \"run\" \"--agent-image\" \"my-image\" \"--work-image\" \"/opt/my tools/work\"
Environment=\"PATH=/usr/bin:/home/ada/100%%\"
Environment=\"XDG_STATE_HOME=/home/ada/.local/state\"
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
"
    );
}

#[test]
fn the_launchd_agent_keeps_the_agent_alive_and_logs_to_the_users_logs() {
    let mut s = service();
    s.args.push("<&>".to_owned());
    let plist = launchd(&s);
    assert_eq!(
        plist,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.moor.machine-agent</string>
  <key>ProgramArguments</key>
  <array>
    <string>/home/ada/.local/bin/moor-machine-agent</string>
    <string>run</string>
    <string>--agent-image</string>
    <string>my-image</string>
    <string>--work-image</string>
    <string>/opt/my tools/work</string>
    <string>&lt;&amp;&gt;</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/bin:/home/ada/100%</string>
    <key>XDG_STATE_HOME</key>
    <string>/home/ada/.local/state</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/home/ada/Library/Logs/moor-machine-agent.log</string>
</dict>
</plist>
"#
    );
}
