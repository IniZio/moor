//! The prelude run under a real `sh` in a temp `HOME`.

use super::*;

/// A fresh directory under the system temp dir, removed when dropped.
struct TempHome(std::path::PathBuf);

impl TempHome {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("moor-home-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let sudo = dir.join("bin/sudo");
        std::fs::write(&sudo, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&sudo, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        Self(dir)
    }

    fn proxy_dir(&self) -> std::path::PathBuf {
        self.0.join(".moor-proxy")
    }

    /// The prelude lines that write the CA and build the bundle, run under `sh` with this `HOME`
    /// and a `sudo` that always fails; the child prints the bundle path the exports use.
    fn spawn(&self) -> std::process::Child {
        let got = build(&ctx(&[], Some(PEM), None), Purpose::Run).unwrap();
        let from = got
            .prelude
            .iter()
            .position(|l| l.starts_with("mkdir -p \"$HOME/.moor-proxy\""))
            .unwrap();
        let to = got
            .prelude
            .iter()
            .position(|l| l.starts_with("if [ -n \"$moor_sys\" ]"))
            .unwrap();
        let mut script = got.prelude.get(from..=to).unwrap().join("\n");
        script.push_str("\nprintf '%s' \"$moor_bundle\"");
        let path = format!(
            "{}:{}",
            self.0.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::process::Command::new("sh")
            .args(["-c", &script])
            .env("HOME", &self.0)
            .env("PATH", path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// The whole wrapped exec of `argv`, run with this `HOME` and a `sudo` that always fails.
    fn exec(&self, argv: Vec<String>) -> std::process::Output {
        let got = build(&ctx(&[], Some(PEM), None), Purpose::Run).unwrap();
        let argv = wrap_argv(&got, argv);
        let path = format!(
            "{}:{}",
            self.0.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::process::Command::new(argv.first().unwrap())
            .args(argv.get(1..).unwrap())
            .env("HOME", &self.0)
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

    fn inode(&self, name: &str) -> u64 {
        std::os::unix::fs::MetadataExt::ino(
            &std::fs::metadata(self.proxy_dir().join(name)).unwrap(),
        )
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ =
            std::fs::set_permissions(&self.0, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.0);
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
            home.0.clone()
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
