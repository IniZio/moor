#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use moor_client::rootless;

use super::{gaps, prepare, Daemon, Facts, Packager, Ran, Shell, QUESTION};

mod daemon;

const SETUPTOOL: &str = "/usr/bin/dockerd-rootless-setuptool.sh";
const ROOTLESS: &str = r#"["name=seccomp,profile=builtin","name=rootless"]"#;

/// A host of user `ada` (uid 1000) under `dir` that is ready, with its rootless daemon up.
fn ready(dir: &Path) -> Facts {
    Facts {
        rootless: rootless::Facts {
            user: "ada".to_owned(),
            uid: 1000,
            subuid: Some("ada:100000:65536\n".to_owned()),
            subgid: Some("ada:100000:65536\n".to_owned()),
            missing: Vec::new(),
            apparmor_restrict: Some("0\n".to_owned()),
            unit: Some("enabled".to_owned()),
            linger: Some(true),
            rootlesskit: Ok(vec!["--disable-host-loopback".to_owned()]),
            live_restore: Some(false),
        },
        kvm_device: "/dev/kvm".to_owned(),
        kvm_mode: Some(0o666),
        packager: Some(Packager::Apt),
        docker_repo: true,
        setuptool: Some(PathBuf::from(SETUPTOOL)),
        daemon: Daemon::Answers,
        home: dir.to_owned(),
        config: dir.join("config"),
        docker_config: dir.join("docker"),
        socket: dir.join("run/docker.sock"),
    }
}

/// One thing a fresh host can lack that only root can give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lack {
    Kvm,
    Uidmap,
    Slirp,
    Subids,
    AppArmor,
    Linger,
    Extras,
}

const LACKS: [Lack; 7] = [
    Lack::Kvm,
    Lack::Uidmap,
    Lack::Slirp,
    Lack::Subids,
    Lack::AppArmor,
    Lack::Linger,
    Lack::Extras,
];

/// `f` without what `lack` names.
fn lacking(mut f: Facts, lack: Lack) -> Facts {
    match lack {
        Lack::Kvm => f.kvm_mode = Some(0o660),
        Lack::Uidmap => f
            .rootless
            .missing
            .extend(["newuidmap".to_owned(), "newgidmap".to_owned()]),
        Lack::Slirp => f.rootless.missing.push("slirp4netns".to_owned()),
        Lack::Subids => {
            f.rootless.subuid = None;
            f.rootless.subgid = Some(String::new());
        }
        Lack::AppArmor => f.rootless.apparmor_restrict = Some("1\n".to_owned()),
        Lack::Linger => f.rootless.linger = Some(false),
        Lack::Extras => {
            f.setuptool = None;
            f.daemon = Daemon::Absent;
        }
    }
    f
}

/// Whether `script`, every command the gaps give, fixes `lack`.
fn fixes(script: &str, lack: Lack) -> bool {
    let install = script
        .lines()
        .find(|l| l.starts_with("apt-get install"))
        .unwrap_or_default();
    let installs = |pkg: &str| install.split_whitespace().any(|w| w == pkg);
    match lack {
        Lack::Kvm => {
            script.contains(r#"echo 'KERNEL=="kvm", MODE="0666"' > /etc/udev/rules.d/65-kvm.rules"#)
        }
        Lack::Uidmap => installs("uidmap"),
        Lack::Slirp => installs("slirp4netns"),
        Lack::Subids => {
            script.contains("usermod --add-subuids 100000-165535 --add-subgids 100000-165535 ada")
        }
        Lack::AppArmor => {
            script.contains("sysctl -w kernel.apparmor_restrict_unprivileged_userns=0")
        }
        Lack::Linger => script.contains("loginctl enable-linger ada"),
        Lack::Extras => installs("docker-ce-rootless-extras"),
    }
}

/// The commands of every gap of `f`, one per line.
fn script(f: &Facts) -> String {
    gaps(f)
        .unwrap()
        .iter()
        .flat_map(|g| g.commands.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_combination_of_root_only_gaps_gets_exactly_its_fixes() {
    let dir = tempfile::tempdir().unwrap();
    for mask in 0_u32..1 << LACKS.len() {
        let lacks: Vec<Lack> = LACKS
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, l)| *l)
            .collect();
        let f = lacks.iter().fold(ready(dir.path()), |f, l| lacking(f, *l));
        let script = script(&f);
        for lack in LACKS {
            assert_eq!(
                fixes(&script, lack),
                lacks.contains(&lack),
                "{lack:?} with {lacks:?}:\n{script}"
            );
        }
        assert_eq!(
            script.contains("apt-get update"),
            script.contains("apt-get install"),
            "{script}"
        );
    }
}

#[test]
fn a_ready_host_has_no_gaps() {
    let dir = tempfile::tempdir().unwrap();
    assert!(gaps(&ready(dir.path())).unwrap().is_empty());
}

#[test]
fn rootless_extras_is_only_needed_for_a_daemon_that_is_not_set_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.setuptool = None;
    assert!(gaps(&f).unwrap().is_empty(), "the daemon already answers");
    f.daemon = Daemon::Absent;
    f.docker_repo = false;
    assert!(
        gaps(&f).unwrap().is_empty(),
        "no Docker repository: Docker's rootless installer runs as the user instead"
    );
}

#[test]
fn a_missing_kvm_device_fails_plainly_and_is_no_sudo_fix() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.kvm_mode = None;
    let err = gaps(&f).unwrap_err();
    assert!(err.contains("/dev/kvm is missing"), "{err}");
    assert!(err.contains("firmware"), "{err}");
}

#[test]
fn packages_on_a_host_without_apt_or_dnf_fail_with_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = lacking(ready(dir.path()), Lack::Uidmap);
    f.packager = None;
    let err = gaps(&f).unwrap_err();
    assert!(err.contains("uidmap"), "{err}");
    assert!(err.contains("package manager"), "{err}");
}

#[test]
fn dnf_hosts_get_dnf_package_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = lacking(lacking(ready(dir.path()), Lack::Uidmap), Lack::Slirp);
    f.packager = Some(Packager::Dnf);
    assert!(
        script(&f).contains("dnf install -y shadow-utils slirp4netns"),
        "{}",
        script(&f)
    );
}

#[test]
fn new_subids_start_after_every_range_already_given() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.rootless.subuid = Some("bob:100000:65536\n".to_owned());
    f.rootless.subgid = Some("bob:100000:65536\ncarol:165536:65536\n".to_owned());
    assert!(
        script(&f).contains("usermod --add-subuids 231072-296607 --add-subgids 231072-296607 ada"),
        "{}",
        script(&f)
    );
}

#[test]
fn only_the_short_id_file_gets_more_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.rootless.subgid = None;
    let s = script(&f);
    assert!(
        s.contains("usermod --add-subgids ") && s.ends_with(" ada"),
        "{s}"
    );
    assert!(!s.contains("--add-subuids"), "{s}");
}

/// A shell that answers each command by the first answer whose prefix it starts with, else
/// with success and no output, and keeps what ran.
struct Fake {
    ran: Vec<String>,
    envs: Vec<Vec<(String, Option<String>)>>,
    answers: Vec<(&'static str, Ran)>,
    sudo: Vec<String>,
    sudo_ok: bool,
    on_run: Box<dyn FnMut(&str)>,
}

impl Fake {
    fn new() -> Self {
        Self {
            ran: Vec::new(),
            envs: Vec::new(),
            answers: vec![(
                "docker info --format {{json .SecurityOptions}}",
                Ran {
                    ok: true,
                    out: ROOTLESS.to_owned(),
                },
            )],
            sudo: Vec::new(),
            sudo_ok: true,
            on_run: Box::new(|_| ()),
        }
    }

    fn answer(mut self, prefix: &'static str, ok: bool, out: &str) -> Self {
        self.answers.insert(
            0,
            (
                prefix,
                Ran {
                    ok,
                    out: out.to_owned(),
                },
            ),
        );
        self
    }

    /// The env changes the command starting with `prefix` ran with.
    fn env_of(&self, prefix: &str) -> Vec<(String, Option<String>)> {
        let i = self
            .ran
            .iter()
            .position(|r| r.starts_with(prefix))
            .expect(prefix);
        self.envs[i].clone()
    }

    fn ran(&self, prefix: &str) -> bool {
        self.ran.iter().any(|r| r.starts_with(prefix))
    }
}

impl Shell for Fake {
    fn run(&mut self, program: &str, args: &[&str], env: &[(&str, Option<&str>)]) -> Ran {
        let line = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        (self.on_run)(&line);
        self.ran.push(line.clone());
        self.envs.push(
            env.iter()
                .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
                .collect(),
        );
        self.answers
            .iter()
            .find(|(p, _)| line.starts_with(p))
            .map_or(
                Ran {
                    ok: true,
                    out: String::new(),
                },
                |(_, r)| r.clone(),
            )
    }

    fn sudo(&mut self, script: &str) -> bool {
        self.sudo.push(script.to_owned());
        self.sudo_ok
    }
}

/// What `prepare` said and returned for a host that reads as `before` until sudo runs and as
/// `after` from then on, with `answer` to the question, or no terminal when none.
fn run(
    before: &Facts,
    after: &Facts,
    shell: &mut Fake,
    answer: Option<bool>,
) -> (Result<(), String>, String, Vec<String>) {
    let sudo_ran = RefCell::new(0_usize);
    let mut said = String::new();
    let mut asked = Vec::new();
    let read = |s: &mut dyn Shell| {
        let _ = s;
        Ok(if *sudo_ran.borrow() == 0 {
            before.clone()
        } else {
            after.clone()
        })
    };
    let mut ask = |q: &str| {
        asked.push(q.to_owned());
        answer.unwrap_or(false)
    };
    let mut counting = Counting {
        inner: shell,
        sudo: &sudo_ran,
    };
    let result = prepare(
        &read,
        &mut counting,
        answer
            .is_some()
            .then_some(&mut ask as &mut dyn FnMut(&str) -> bool),
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    (result, said, asked)
}

/// Counts sudo runs, so the host can read as fixed after one.
struct Counting<'a> {
    inner: &'a mut Fake,
    sudo: &'a RefCell<usize>,
}

impl Shell for Counting<'_> {
    fn run(&mut self, program: &str, args: &[&str], env: &[(&str, Option<&str>)]) -> Ran {
        self.inner.run(program, args, env)
    }

    fn sudo(&mut self, script: &str) -> bool {
        *self.sudo.borrow_mut() += 1;
        self.inner.sudo(script)
    }
}

#[test]
fn a_ready_host_says_ok_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut sh = Fake::new();
    let (result, said, asked) = run(&ready(dir.path()), &ready(dir.path()), &mut sh, Some(true));
    result.unwrap();
    assert_eq!(said, "host: ok\nrootless docker: ok\n");
    assert!(asked.is_empty());
    assert!(sh.sudo.is_empty());
    assert!(!sh.ran("systemctl") && !sh.ran(SETUPTOOL), "{:?}", sh.ran);
    assert!(!dir.path().join("config").exists());
}

#[test]
fn without_a_terminal_up_prints_the_one_sudo_command_and_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(lacking(ready(dir.path()), Lack::Kvm), Lack::Subids);
    let mut sh = Fake::new();
    let (result, said, _) = run(&f, &f, &mut sh, None);
    let err = result.unwrap_err();
    assert!(sh.sudo.is_empty());
    assert!(said.contains("needs root for"), "{said}");
    assert!(said.contains("/dev/kvm is mode 0660"), "{said}");
    assert!(said.contains("sudo sh -ec '"), "{said}");
    assert!(
        said.contains(r#"echo '\''KERNEL=="kvm", MODE="0666"'\'' >"#),
        "{said}"
    );
    assert!(said.contains("usermod --add-subuids"), "{said}");
    assert!(err.contains("no terminal"), "{err}");
    assert!(
        err.contains("run the sudo command above, then run moor-machine-agent up again"),
        "{err}"
    );
}

#[test]
fn on_a_terminal_up_asks_once_and_runs_every_fix_in_one_sudo() {
    let dir = tempfile::tempdir().unwrap();
    let f = LACKS.iter().fold(ready(dir.path()), |f, l| lacking(f, *l));
    let mut sh = Fake::new().answer("loginctl enable-linger", false, "");
    let after = ready(dir.path());
    let (result, said, asked) = run(&f, &after, &mut sh, Some(true));
    result.unwrap();
    assert_eq!(asked, [QUESTION]);
    assert_eq!(sh.sudo.len(), 1);
    for lack in LACKS {
        assert!(fixes(&sh.sudo[0], lack), "{lack:?}: {}", sh.sudo[0]);
    }
    assert!(said.contains("host: fixed with sudo"), "{said}");
}

#[test]
fn saying_no_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(ready(dir.path()), Lack::Kvm);
    let mut sh = Fake::new();
    let (result, _, asked) = run(&f, &f, &mut sh, Some(false));
    let err = result.unwrap_err();
    assert_eq!(asked.len(), 1);
    assert!(sh.sudo.is_empty());
    assert!(err.contains("run the sudo command above"), "{err}");
}

#[test]
fn a_failed_sudo_stops_up() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(ready(dir.path()), Lack::Kvm);
    let mut sh = Fake::new();
    sh.sudo_ok = false;
    let (result, _, _) = run(&f, &ready(dir.path()), &mut sh, Some(true));
    let err = result.unwrap_err();
    assert!(err.contains("sudo failed"), "{err}");
    assert!(!sh.ran(SETUPTOOL));
}

#[test]
fn what_sudo_left_undone_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(ready(dir.path()), Lack::Kvm);
    let mut sh = Fake::new();
    let (result, _, _) = run(&f, &f, &mut sh, Some(true));
    let err = result.unwrap_err();
    assert!(err.contains("still"), "{err}");
    assert!(err.contains("/dev/kvm"), "{err}");
}

#[test]
fn linger_is_turned_on_without_sudo_when_loginctl_allows_it() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(ready(dir.path()), Lack::Linger);
    let mut sh = Fake::new();
    let (result, said, asked) = run(&f, &f, &mut sh, None);
    result.unwrap();
    assert!(sh.ran("loginctl enable-linger"), "{:?}", sh.ran);
    assert!(asked.is_empty() && sh.sudo.is_empty());
    assert!(said.starts_with("linger: turned on\nhost: ok\n"), "{said}");
}

#[test]
fn linger_goes_in_the_sudo_command_when_loginctl_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let f = lacking(ready(dir.path()), Lack::Linger);
    let mut sh = Fake::new().answer("loginctl enable-linger", false, "Access denied");
    let (result, said, _) = run(&f, &f, &mut sh, None);
    assert!(result.is_err());
    assert!(said.contains("loginctl enable-linger ada"), "{said}");
}
