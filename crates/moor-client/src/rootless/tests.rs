#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use super::{
    cmdline_words, failures, is_rootless, loopback_closed, notice, report, state_dir, warnings,
    Facts, Failure,
};

/// What rootless Docker 29.10 printed for its security options.
const ROOTLESS: &str = r#"["name=seccomp,profile=builtin","name=rootless","name=cgroupns"]"#;
/// What the system daemon printed on the same host.
const ROOTFUL: &str =
    r#"["name=apparmor,profile=default","name=seccomp,profile=builtin","name=cgroupns"]"#;
/// rootlesskit's child's command line, as `/proc/<pid>/cmdline` held it.
const CMDLINE: &[u8] = b"/proc/self/exe\0--state-dir=/run/user/1003/rl/dockerd-rootless\0--net=slirp4netns\0--disable-host-loopback\0--port-driver=builtin\0/usr/bin/dockerd-rootless.sh\0-H\0unix:///run/user/1003/rl/docker.sock\0";

/// A host where every check passes, for user `ada` (uid 1000).
fn good() -> Facts {
    Facts {
        user: "ada".to_owned(),
        uid: 1000,
        subuid: Some("ada:100000:65536\n".to_owned()),
        subgid: Some("ada:100000:65536\n".to_owned()),
        missing: Vec::new(),
        apparmor_restrict: Some("0\n".to_owned()),
        unit: Some("enabled".to_owned()),
        linger: Some(true),
        rootlesskit: Ok(cmdline_words(CMDLINE)),
        kvm_mode: Some(0o666),
    }
}

/// The checks that fail for `facts`, by name.
fn names(facts: &Facts, local: bool) -> Vec<&'static str> {
    failures(facts, local).iter().map(|f| f.check).collect()
}

/// The one failure of `facts`.
fn only(facts: &Facts) -> Failure {
    let mut all = failures(facts, true);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

#[test]
fn only_name_rootless_in_the_security_options_makes_docker_rootless() {
    assert!(is_rootless(ROOTLESS));
    assert!(!is_rootless(ROOTFUL));
    assert!(!is_rootless("null"));
    assert!(!is_rootless("27.0.1"));
    assert!(!is_rootless(r#"["name=rootless-ish"]"#));
}

#[test]
fn a_host_that_is_ready_has_no_failures() {
    assert_eq!(failures(&good(), true), []);
}

#[test]
fn too_few_subordinate_ids_name_usermod() {
    let mut f = good();
    f.subuid = Some("ada:100000:1000\nbob:200000:65536\n".to_owned());
    f.subgid = None;
    let all = failures(&f, true);
    assert_eq!(names(&f, true), ["subid", "subid"]);
    assert!(all[0].fix.contains("usermod --add-subuids"), "{all:?}");
    assert!(all[1].fix.contains("usermod --add-subgids"), "{all:?}");
}

#[test]
fn ranges_add_up_and_a_uid_names_the_user_too() {
    let mut f = good();
    f.subuid = Some("ada:100000:40000\n1000:200000:30000\n".to_owned());
    assert_eq!(failures(&f, true), []);
}

#[test]
fn a_missing_program_names_its_package() {
    let mut f = good();
    f.missing = ["newuidmap", "newgidmap", "slirp4netns"]
        .map(str::to_owned)
        .to_vec();
    let all = failures(&f, true);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].fix, "install uidmap");
    assert_eq!(all[2].fix, "install slirp4netns");
}

#[test]
fn nsenter_matters_only_where_a_relay_enters_the_namespace() {
    let mut f = good();
    f.missing = vec!["nsenter".to_owned()];
    assert_eq!(only(&f).fix, "install util-linux");
    assert_eq!(names(&f, false), Vec::<&str>::new());
}

#[test]
fn apparmor_restricting_user_namespaces_fails_and_a_host_without_the_sysctl_passes() {
    let mut f = good();
    f.apparmor_restrict = Some("1\n".to_owned());
    assert!(only(&f)
        .fix
        .contains("apparmor_restrict_unprivileged_userns=0"));
    f.apparmor_restrict = None;
    assert_eq!(failures(&f, true), []);
}

#[test]
fn a_unit_that_is_not_enabled_names_systemctl() {
    for state in [Some("not-found"), Some("disabled"), Some(""), None] {
        let mut f = good();
        f.unit = state.map(str::to_owned);
        assert_eq!(
            only(&f).fix,
            "run `systemctl --user enable docker`",
            "{state:?}"
        );
    }
}

#[test]
fn a_user_who_does_not_linger_names_loginctl_and_an_unknown_one_passes() {
    let mut f = good();
    f.linger = Some(false);
    assert_eq!(only(&f).fix, "run `loginctl enable-linger`");
    f.linger = None;
    assert_eq!(failures(&f, true), []);
}

#[test]
fn rootlesskit_without_the_loopback_flag_names_the_variable() {
    let mut f = good();
    f.rootlesskit = Ok(vec![
        "rootlesskit".to_owned(),
        "--net=slirp4netns".to_owned(),
    ]);
    let one = only(&f);
    assert_eq!(one.check, "loopback");
    assert!(one
        .fix
        .contains("DOCKERD_ROOTLESS_ROOTLESSKIT_DISABLE_HOST_LOOPBACK"));
    f.rootlesskit = Err("cannot read /run/user/1000/dockerd-rootless/child_pid".to_owned());
    assert!(only(&f).detail.contains("child_pid"));
}

#[test]
fn the_loopback_flag_may_carry_true_and_never_false() {
    let words = |w: &str| vec![w.to_owned()];
    assert!(loopback_closed(&words("--disable-host-loopback")));
    assert!(loopback_closed(&words("--disable-host-loopback=true")));
    assert!(!loopback_closed(&words("--disable-host-loopback=false")));
    assert!(!loopback_closed(&[]));
}

#[test]
fn a_kvm_device_not_open_to_everyone_is_a_warning_not_a_failure() {
    let mut f = good();
    assert_eq!(warnings(&f), []);
    f.kvm_mode = Some(0o660);
    assert_eq!(failures(&f, true), []);
    let [one] = warnings(&f).try_into().unwrap();
    assert_eq!(one.check, "kvm");
    assert!(
        one.detail.starts_with(
            "KVM Boxes (Android emulators) will not work on this node until /dev/kvm is 0666"
        ),
        "{one:?}"
    );
    assert!(one.fix.contains("0666"), "{one:?}");
    f.kvm_mode = None;
    assert_eq!(warnings(&f), []);
}

#[test]
fn every_failure_is_reported_at_once() {
    let mut f = good();
    f.linger = Some(false);
    f.kvm_mode = Some(0o600);
    f.unit = None;
    assert_eq!(names(&f, true), ["docker.service", "linger"]);
}

#[test]
fn the_report_gives_each_failure_a_line_with_its_fix() {
    let mut f = good();
    f.linger = Some(false);
    f.rootlesskit = Ok(Vec::new());
    let text = report(&failures(&f, true));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[0], "rootless Docker is not ready for Boxes:");
    assert!(
        lines[1].starts_with("- linger: ada does not linger"),
        "{text}"
    );
    assert!(
        lines[1].ends_with("; fix: run `loginctl enable-linger`"),
        "{text}"
    );
    assert!(lines[2].starts_with("- loopback: "), "{text}");
}

#[test]
fn the_state_dir_sits_beside_the_daemons_socket() {
    assert_eq!(
        state_dir("unix:///run/user/1003/rl/docker.sock").unwrap(),
        std::path::Path::new("/run/user/1003/rl/dockerd-rootless")
    );
    let err = state_dir("tcp://10.0.0.2:2375").unwrap_err();
    assert!(err.contains("not a Unix socket"), "{err}");
}

#[test]
fn a_proc_command_line_splits_on_nul() {
    let words = cmdline_words(CMDLINE);
    assert_eq!(words[0], "/proc/self/exe");
    assert!(loopback_closed(&words));
    assert_eq!(
        words.last().unwrap(),
        "unix:///run/user/1003/rl/docker.sock"
    );
}

#[test]
fn the_notice_names_the_user_each_time_the_spec_does() {
    let text = notice("ada");
    assert!(text.starts_with("Docker Boxes run on rootless Docker as ada. Each Box's work place runs with --privileged so Docker works inside it, and a Box that breaks out is ada on this host, not root. It can reach ada's files and every other Box here."), "{text}");
}
