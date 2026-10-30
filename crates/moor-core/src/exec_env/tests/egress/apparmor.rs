//! The egress lines against a fake `AppArmor` tinyproxy profile: local rules, reload, and the
//! cases that leave `AppArmor` alone.

use std::path::Path;

use super::{failure, Apt, FakeBox};

impl FakeBox {
    /// Installs a tinyproxy profile the kernel lists as `loaded`, and an `apparmor_parser` that
    /// fails when `reloads` is false.
    fn apparmor(&self, loaded: &str, reloads: bool) {
        let [profile, _, _, list] = self.apparmor_paths();
        std::fs::create_dir_all(self.home.join("aa")).unwrap();
        std::fs::write(profile, "profile tinyproxy /usr/bin/tinyproxy {}\n").unwrap();
        std::fs::write(list, format!("{loaded}\n")).unwrap();
        let exit = if reloads {
            ""
        } else {
            "echo 'AppArmor parser error: syntax error'\nexit 1\n"
        };
        moor_testkit::write_executable(
            &self.bin.join("apparmor_parser"),
            format!("#!/bin/sh\necho \"apparmor_parser $*\" >>\"$HOME/fake.log\"\n{exit}"),
        )
        .unwrap();
    }

    fn local_rules(&self) -> Option<String> {
        std::fs::read_to_string(&self.apparmor_paths()[2]).ok()
    }

    fn reloads(&self) -> Vec<String> {
        self.log()
            .lines()
            .filter(|l| l.starts_with("apparmor_parser"))
            .map(str::to_owned)
            .collect()
    }
}

/// The local rules for the Box whose home is `home`: its own `~/.moor` files and nothing more.
fn rules(home: &Path) -> String {
    let m = home.join(".moor");
    let m = m.display();
    format!(
        "# Written by moor for its egress proxy.\nfile r \"{m}/egress.conf\",\nfile r \"{m}/egress.conf.new\",\nfile rw \"{m}/egress.pid\",\nfile w \"{m}/egress-start.log\",\n"
    )
}

#[test]
fn an_enforced_tinyproxy_profile_allows_the_box_moor_files_before_tinyproxy_starts() {
    let b = FakeBox::new(0, Apt::Works, true);
    b.apparmor("tinyproxy (enforce)", true);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.local_rules().as_deref(), Some(rules(&b.home).as_str()));
    let log = b.log();
    let order: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("apparmor_parser") || l.starts_with("tinyproxy "))
        .collect();
    assert_eq!(order.len(), 2, "{log}");
    assert_eq!(
        order.first().copied(),
        Some(format!("apparmor_parser -r {}", b.apparmor_paths()[0]).as_str()),
        "{log}"
    );
    assert!(!log.contains("sudo"), "{log}");
}

#[test]
fn unchanged_local_rules_are_not_reloaded_when_tinyproxy_starts_again() {
    let b = FakeBox::new(0, Apt::Works, true);
    b.apparmor("tinyproxy (enforce)", true);
    assert!(b.run().0);
    b.kill();
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "{}", b.log());
    assert_eq!(b.reloads().len(), 1, "{}", b.log());
}

#[test]
fn a_non_root_box_writes_the_local_rules_through_sudo() {
    let b = FakeBox::new(1000, Apt::Works, true);
    b.apparmor("tinyproxy (enforce)", true);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.local_rules().as_deref(), Some(rules(&b.home).as_str()));
    assert!(b.log().starts_with("sudo\n"), "{}", b.log());
    assert_eq!(b.reloads().len(), 1, "{}", b.log());
}

#[test]
fn without_root_or_sudo_an_enforced_profile_stops_the_exec_and_tinyproxy_never_starts() {
    let b = FakeBox::new(1000, Apt::Works, true);
    b.apparmor("tinyproxy (enforce)", true);
    moor_testkit::write_executable(
        &b.bin.join("sudo"),
        "#!/bin/sh\necho 'sudo: a password is required' >&2\nexit 1\n",
    )
    .unwrap();
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    let [profile, _, local, _] = b.apparmor_paths();
    let home = b.home.display();
    assert!(
        err.contains(&failure(&format!(
            "{profile} confines tinyproxy, and allowing it {home}/.moor in {local} needs root or passwordless sudo"
        ))),
        "{err}"
    );
    assert_eq!(b.starts(), 0);
    assert_eq!(b.local_rules(), None);
}

#[test]
fn a_failed_reload_removes_the_local_rules_and_stops_the_exec_with_the_parser_line() {
    let b = FakeBox::new(0, Apt::Works, true);
    b.apparmor("tinyproxy (enforce)", false);
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    let profile = &b.apparmor_paths()[0];
    let home = b.home.display();
    assert!(
        err.contains(&failure(&format!(
            "{profile} could not be reloaded with {home}/.moor allowed: AppArmor parser error: syntax error"
        ))),
        "{err}"
    );
    assert_eq!(b.starts(), 0);
    assert_eq!(b.local_rules(), None);
}

#[test]
fn no_profile_a_profile_not_enforced_or_a_tinyproxy_it_does_not_attach_to_changes_nothing() {
    let none = FakeBox::new(0, Apt::Works, true);
    let complain = FakeBox::new(0, Apt::Works, true);
    complain.apparmor("tinyproxy (complain)", true);
    let elsewhere = FakeBox::new(0, Apt::Works, true);
    elsewhere.apparmor("tinyproxy (enforce)", true);
    let other = elsewhere.home.join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::rename(elsewhere.bin.join("tinyproxy"), other.join("tinyproxy")).unwrap();
    std::os::unix::fs::symlink(other.join("tinyproxy"), elsewhere.bin.join("tinyproxy")).unwrap();
    for b in [&none, &complain, &elsewhere] {
        let (ok, out, err) = b.run();
        assert!(ok, "{out}{err}");
        assert_eq!(b.local_rules(), None);
        assert_eq!(b.reloads(), Vec::<String>::new());
        assert_eq!(b.starts(), 1, "{}", b.log());
    }
}
