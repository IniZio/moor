use super::Service;

fn service() -> Service {
    Service {
        exe: "/home/a b/.cargo/bin/moor-hc".to_owned(),
        args: vec![
            "sync".to_owned(),
            "--server".to_owned(),
            "https://moor.example.com".to_owned(),
        ],
        path: "/usr/bin:/bin".to_owned(),
    }
}

#[test]
fn linux_gets_a_systemd_unit_that_spares_hcs_core() {
    let (unit, how) = service().render("linux");
    assert!(
        unit.contains(
            "ExecStart=\"/home/a b/.cargo/bin/moor-hc\" \"sync\" \"--server\" \"https://moor.example.com\"\n"
        ),
        "{unit}"
    );
    assert!(unit.contains("KillMode=process\n"), "{unit}");
    assert!(
        unit.contains("Environment=\"PATH=/usr/bin:/bin\"\n"),
        "{unit}"
    );
    assert!(
        how.contains("systemctl --user enable --now moor-hc"),
        "{how}"
    );
}

#[test]
fn macos_gets_a_launchd_agent() {
    let (plist, how) = service().render("macos");
    assert!(
        plist.contains("<string>/home/a b/.cargo/bin/moor-hc</string>\n    <string>sync</string>"),
        "{plist}"
    );
    assert!(plist.contains("<key>AbandonProcessGroup</key>"), "{plist}");
    assert!(how.contains("launchctl load -w"), "{how}");
}
