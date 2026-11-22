//! `moor up` and `moor doctor` on a host whose registry has no image of this build.

use std::io::Write;
use std::process::Stdio;

use moor_testkit::ChildGuard;

use super::up::Host;
use super::Run;

impl Host {
    pub(super) fn moor_with(&self, args: &[&str], stdin: &str, vars: &[(&str, &str)]) -> Run {
        if args.first() == Some(&"up") {
            self.keep_ports();
        }
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_moor"))
            .args(args)
            .current_dir(&self.home)
            .env_clear()
            .env("MOOR_KVM_DEVICE", self.bin.join("kvm"))
            .env("MOOR_HOST_ROOT", self.root())
            .envs(vars.iter().copied())
            .env("PATH", self.path())
            .env("HOME", &self.home)
            .env("MOOR_UP_LINK_WAIT_SECS", "0")
            .env("MOOR_STATE_DIR", self.state.path())
            .env(
                "MOOR_EGRESS_FORWARD_BIN",
                self.bin.join("moor-egress-forward"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = ChildGuard::new(child);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        Run {
            code: out.status.code().unwrap(),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }
}

/// The `docker` answers of a daemon that has no image of this build, nor its registry.
const NO_IMAGES: &str = r#"  "info --format {{.ServerVersion}}") echo 29.10.0 ;;
  "image inspect "*) echo "Error: No such image" >&2; exit 1 ;;
  "manifest inspect "*) echo "no such manifest: $3" >&2; exit 1 ;;
"#;

#[test]
fn up_warns_and_doctor_fails_when_the_agents_default_images_do_not_exist() {
    let host = Host::new();
    host.docker(NO_IMAGES);
    let up = host.moor(&["up"], "sk-claude\ngh-token\n");
    assert_eq!(up.code, 0, "up only warns: {up:?}");
    let warned = up
        .stdout
        .lines()
        .filter(|l| l.starts_with("warn  images"))
        .count();
    assert_eq!(warned, 2, "{up:?}");
    assert!(
        up.stdout
            .contains("set MOOR_AGENT_IMAGE and MOOR_WORK_IMAGE"),
        "{up:?}"
    );
    assert!(up.stdout.contains("ghcr.io/inizio/moor-agent:"), "{up:?}");
    let doctor = host.moor(&["doctor"], "");
    assert_eq!(doctor.code, 1, "{doctor:?}");
    assert_eq!(
        doctor
            .stdout
            .lines()
            .filter(|l| l.starts_with("fail  images"))
            .count(),
        2,
        "{doctor:?}"
    );
}

#[test]
fn an_image_override_up_copied_into_the_agent_unit_is_what_the_doctor_checks() {
    let host = Host::new();
    host.docker(NO_IMAGES);
    let up = host.moor_with(
        &["up"],
        "sk-claude\ngh-token\n",
        &[
            ("MOOR_AGENT_IMAGE", "agent:dev"),
            ("MOOR_WORK_IMAGE", "work:dev"),
        ],
    );
    assert_eq!(up.code, 0, "{up:?}");
    assert!(
        up.stdout
            .contains("the agent image agent:dev is not on this host"),
        "{up:?}"
    );
    assert!(
        up.stdout
            .contains("the work image work:dev is not on this host"),
        "{up:?}"
    );
    let doctor = host.moor(&["doctor"], "");
    assert!(doctor.stdout.contains("fail  images"), "{doctor:?}");
    assert!(doctor.stdout.contains("agent:dev"), "{doctor:?}");
}
