//! The prelude run under a real `sh` in a temp `HOME`.

use super::*;

/// A fresh directory under the system temp dir, removed when dropped, with a fake tinyproxy whose
/// start opens `port`.
struct TempHome(tempfile::TempDir, moor_testkit::FakeEgress);

impl TempHome {
    fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("moor-home-")
            .tempdir()
            .unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let sudo = dir.join("bin/sudo");
        moor_testkit::write_executable(&sudo, "#!/bin/sh\nexit 1\n").unwrap();
        moor_testkit::write_host_guards(&dir.join("bin")).unwrap();
        let egress = moor_testkit::FakeEgress::start(&dir, &dir.join("bin")).unwrap();
        Self(tmp, egress)
    }

    fn proxy_dir(&self) -> std::path::PathBuf {
        self.0.path().join(".moor-proxy")
    }

    /// The prelude lines that write the CA and build the bundle, run under `sh` with this `HOME`,
    /// umask 077 and a `sudo` that always fails; the child prints the bundle path the exports use.
    fn spawn(&self) -> std::process::Child {
        self.spawn_ca(PEM)
    }

    /// [`Self::spawn`] with `pem` as the CA.
    fn spawn_ca(&self, pem: &str) -> std::process::Child {
        let got = build(&ctx(&[], Some(pem), None), Purpose::Run).unwrap();
        let from = got
            .prelude
            .iter()
            .position(|l| l.starts_with("moor_ca="))
            .unwrap();
        let to = got
            .prelude
            .iter()
            .position(|l| l.starts_with("if [ -n \"$moor_sys\" ]"))
            .unwrap();
        let mut script = "umask 077\n".to_owned() + &got.prelude.get(from..=to).unwrap().join("\n");
        script.push_str("\nprintf '%s' \"$moor_bundle\"");
        let path = format!(
            "{}:{}",
            self.0.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::process::Command::new("sh")
            .args(["-c", &script])
            .env("HOME", self.0.path())
            .env("PATH", path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// The whole wrapped exec of `argv`, run with this `HOME` and a `sudo` that always fails.
    fn exec(&self, argv: Vec<String>) -> std::process::Output {
        let mut b = ctx(&[], Some(PEM), None);
        b.proxy = b.proxy.map(|p| ProxyCtx {
            egress_port: self.1.port(),
            ..p
        });
        let got = build(&b, Purpose::Run).unwrap();
        let argv = wrap_argv(&got, argv);
        let path = format!(
            "{}:{}",
            self.0.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::process::Command::new(argv.first().unwrap())
            .args(argv.get(1..).unwrap())
            .env("HOME", self.0.path())
            .env("PATH", path)
            .output()
            .unwrap()
    }

    fn run(&self) -> String {
        let out = self.spawn().wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    /// The names in the proxy directory, sorted.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.proxy_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.proxy_dir().join(name)).unwrap()
    }

    fn mode(&self, name: &str) -> u32 {
        std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(self.proxy_dir().join(name))
                .unwrap()
                .permissions(),
        ) & 0o777
    }

    fn inode(&self, name: &str) -> u64 {
        std::os::unix::fs::MetadataExt::ino(
            &std::fs::metadata(self.proxy_dir().join(name)).unwrap(),
        )
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(
            self.0.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        );
    }
}

#[test]
fn the_prelude_writes_the_ca_and_bundle_under_home_with_no_temp_file_left() {
    let home = TempHome::new();
    let bundle = home.run();
    assert_eq!(
        bundle,
        home.proxy_dir().join("ca-bundle.pem").display().to_string()
    );
    assert_eq!(home.names(), ["ca-bundle.pem", "ca.pem"]);
    assert_eq!(home.read("ca.pem"), PEM);
    assert!(home.read("ca-bundle.pem").ends_with(PEM));
}

#[test]
fn the_ca_and_bundle_are_readable_by_all_under_a_private_umask() {
    let home = TempHome::new();
    home.run();
    assert_eq!(
        (home.mode("ca.pem"), home.mode("ca-bundle.pem")),
        (0o644, 0o644)
    );
    std::fs::write(home.proxy_dir().join("ca.pem"), "stale").unwrap();
    home.run();
    assert_eq!(home.mode("ca.pem"), 0o644);
}

#[test]
fn a_second_prelude_keeps_an_unchanged_ca_and_replaces_a_stale_one() {
    let home = TempHome::new();
    home.run();
    let (ca, bundle) = (home.inode("ca.pem"), home.inode("ca-bundle.pem"));
    home.run();
    assert_eq!(
        (home.inode("ca.pem"), home.inode("ca-bundle.pem")),
        (ca, bundle)
    );
    std::fs::write(home.proxy_dir().join("ca.pem"), "stale").unwrap();
    home.run();
    assert_eq!(home.read("ca.pem"), PEM);
    assert_eq!(home.names(), ["ca-bundle.pem", "ca.pem"]);
}

#[test]
fn preludes_running_at_once_leave_one_whole_ca() {
    let home = TempHome::new();
    for _ in 0..10 {
        let children: Vec<_> = (0..4).map(|_| home.spawn()).collect();
        for child in children {
            assert!(child.wait_with_output().unwrap().status.success());
        }
        assert_eq!(home.read("ca.pem"), PEM);
        assert!(home.read("ca-bundle.pem").ends_with(PEM));
        assert_eq!(home.names(), ["ca-bundle.pem", "ca.pem"]);
    }
}

#[test]
fn an_unwritable_home_or_proxy_dir_stops_the_exec_before_the_command_and_names_the_ca_path() {
    for proxy_dir_exists in [false, true] {
        let home = TempHome::new();
        let locked = if proxy_dir_exists {
            std::fs::create_dir(home.proxy_dir()).unwrap();
            home.proxy_dir()
        } else {
            home.0.path().to_path_buf()
        };
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o555))
            .unwrap();
        let out = home.exec(vec!["echo".to_owned(), "the command ran".to_owned()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
        assert!(!stdout.contains("the command ran"), "{stdout}");
        let ca = home.proxy_dir().join("ca.pem");
        let message = format!("the Proxy CA could not be written to {}", ca.display());
        assert!(stderr.contains(&message), "{stderr}");
    }
}

#[test]
fn a_writable_home_runs_the_command_after_the_prelude() {
    let home = TempHome::new();
    let out = home.exec(vec!["echo".to_owned(), "the command ran".to_owned()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "the command ran\n");
    assert_eq!(home.read("ca.pem"), PEM);
}

/// The exit status, stdout and stderr of a wrapped `echo` run with `home`.
fn echo_in(home: &TempHome) -> (Option<i32>, String, String) {
    let out = home.exec(vec!["echo".to_owned(), "the command ran".to_owned()]);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Asserts that the exec stopped before the command, naming `name` in the proxy directory.
fn assert_stopped_naming(home: &TempHome, name: &str) {
    let (code, stdout, stderr) = echo_in(home);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(!stdout.contains("the command ran"), "{stdout}");
    let file = home.proxy_dir().join(name);
    let message = format!("the Proxy CA could not be written to {}", file.display());
    assert!(stderr.contains(&message), "{stderr}");
}

#[test]
fn a_directory_or_symlink_at_the_ca_or_bundle_stops_the_exec_and_is_left_alone() {
    for name in ["ca.pem", "ca-bundle.pem"] {
        for plant in ["dir", "link to file", "link to dir"] {
            let home = TempHome::new();
            std::fs::create_dir_all(home.proxy_dir()).unwrap();
            let outside = home.0.path().join("outside");
            let at = home.proxy_dir().join(name);
            match plant {
                "dir" => std::fs::create_dir(&at).unwrap(),
                "link to file" => {
                    std::fs::write(&outside, "outside").unwrap();
                    std::os::unix::fs::symlink(&outside, &at).unwrap();
                }
                _ => {
                    std::fs::create_dir(&outside).unwrap();
                    std::os::unix::fs::symlink(&outside, &at).unwrap();
                }
            }
            assert_stopped_naming(&home, name);
            let kept = std::fs::symlink_metadata(&at).unwrap().file_type();
            assert_eq!(kept.is_dir(), plant == "dir", "{name} {plant}");
            assert_eq!(kept.is_symlink(), plant != "dir", "{name} {plant}");
            if plant == "link to file" {
                assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
            }
            if plant == "link to dir" {
                assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
            }
        }
    }
}

#[test]
fn a_bundle_that_cannot_be_written_stops_the_exec_before_the_command() {
    let home = TempHome::new();
    home.run();
    std::fs::remove_file(home.proxy_dir().join("ca-bundle.pem")).unwrap();
    std::fs::set_permissions(
        home.proxy_dir(),
        std::os::unix::fs::PermissionsExt::from_mode(0o555),
    )
    .unwrap();
    assert_stopped_naming(&home, "ca-bundle.pem");
}

#[test]
fn a_bundle_write_that_fails_part_way_stops_the_exec_and_leaves_no_temp_file() {
    let home = TempHome::new();
    let cat = home.0.path().join("bin/cat");
    moor_testkit::write_executable(&cat, "#!/bin/sh\nprintf partial\nexit 1\n").unwrap();
    assert_stopped_naming(&home, "ca-bundle.pem");
    assert_eq!(home.names(), ["ca.pem"]);
}

#[test]
fn an_unchanged_ca_and_bundle_are_kept_without_writing_a_temp_file() {
    let home = TempHome::new();
    home.run();
    std::fs::set_permissions(
        home.proxy_dir(),
        std::os::unix::fs::PermissionsExt::from_mode(0o555),
    )
    .unwrap();
    let (code, stdout, stderr) = echo_in(&home);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert_eq!(stdout, "the command ran\n");
}

#[test]
fn a_reader_never_sees_a_partial_ca_while_preludes_alternate_two_cas() {
    let pem = |c: char| {
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            c.to_string().repeat(40_000)
        )
    };
    let (a, b) = (pem('A'), pem('B'));
    let home = TempHome::new();
    assert!(home.spawn_ca(&a).wait().unwrap().success());
    let ca = home.proxy_dir().join("ca.pem");
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (reads, partial) = std::thread::scope(|s| {
        let reader = s.spawn(|| {
            let (mut reads, mut partial) = (0, 0);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let got = std::fs::read_to_string(&ca).unwrap();
                reads += 1;
                if got != a && got != b {
                    partial += 1;
                }
            }
            (reads, partial)
        });
        let writers: Vec<_> = (0..6)
            .map(|i| {
                let pem = if i % 2 == 0 { &a } else { &b };
                let home = &home;
                s.spawn(move || {
                    for _ in 0..15 {
                        assert!(home.spawn_ca(pem).wait().unwrap().success());
                    }
                })
            })
            .collect();
        // The reader stops before any writer's panic is raised, so a failed write ends the test.
        let mut all_wrote = true;
        for w in writers {
            all_wrote &= w.join().is_ok();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let read = reader.join().unwrap();
        assert!(all_wrote, "a prelude failed to write the CA");
        read
    });
    assert!(reads > 0);
    assert_eq!(partial, 0, "{partial} of {reads} reads saw a partial CA");
    assert_eq!(home.names(), ["ca-bundle.pem", "ca.pem"]);
}

#[test]
fn keytool_imports_the_ca_the_prelude_just_wrote() {
    let home = TempHome::new();
    let keytool = home.0.path().join("bin/keytool");
    moor_testkit::write_executable(
        &keytool,
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -file) cat \"$2\" > \"$HOME/imported.pem\";;\n    -keystore|-destkeystore) : >> \"$2\";;\n  esac\n  shift\ndone\n",
    )
    .unwrap();
    let (code, stdout, stderr) = echo_in(&home);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(home.0.path().join("imported.pem")).unwrap(),
        PEM
    );
}
