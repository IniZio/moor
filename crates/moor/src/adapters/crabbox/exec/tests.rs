//! Box commands over a fake ssh that runs the remote command on this host.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::driver::{BoxStep, Driver, DriverError, ExecOutcome, ExecSpec, StepDone};
use moor_core::seam::proc::{Deadline, Output, ProcError};

use crate::adapters::crabbox::tests::fake_ssh::{driver, handle, lab, tools, Lab};

impl Lab {
    fn exec(&self, spec: &ExecSpec) -> Result<ExecOutcome, DriverError> {
        self.cb
            .exec(&handle(), spec, Deadline(Duration::from_secs(20)))
    }

    fn log(&self) -> PathBuf {
        self.dir.path().join("out.log")
    }
}

fn sh(script: &str) -> ExecSpec {
    ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env: Vec::new(),
        cwd: None,
        output: None,
    }
}

fn streamed(lab: &Lab, script: &str) -> ExecSpec {
    ExecSpec {
        output: Some(Output::Log(lab.log())),
        ..sh(script)
    }
}

#[test]
fn commands_ride_the_box_master_with_crabbox_host_key_pinning_and_spawn_crabbox_once() {
    let lab = lab();
    for _ in 0..3 {
        assert_eq!(lab.exec(&sh("true")), Ok(ExecOutcome::Exited { code: 0 }));
    }
    let crabbox = lab.argvs("crabbox");
    assert_eq!(crabbox.len(), 1, "only the target lookup: {crabbox:?}");
    assert_eq!(crabbox.first().unwrap().first().unwrap(), "ssh");
    let ssh = lab.argvs("ssh");
    assert_eq!(ssh.len(), 3);
    for argv in &ssh {
        lab.assert_on_the_master_and_pinned(argv);
    }
}

#[test]
fn a_fresh_driver_reuses_the_saved_target_and_spawns_no_crabbox_for_exec_or_the_workdir_probe() {
    let lab = lab();
    assert_eq!(lab.exec(&sh("true")), Ok(ExecOutcome::Exited { code: 0 }));
    let fresh = driver(lab.dir.path(), &tools(&lab.bin()));
    let spec = sh("true");
    let deadline = Deadline(Duration::from_secs(20));
    assert_eq!(
        fresh.exec(&handle(), &spec, deadline),
        Ok(ExecOutcome::Exited { code: 0 })
    );
    fs::write(lab.bin().join("drop"), "").unwrap();
    assert!(matches!(
        fresh.workdir(&handle()),
        Err(DriverError::Transport { .. })
    ));
    assert_eq!(
        lab.argvs("crabbox").len(),
        1,
        "the first driver's lookup only"
    );
    let ssh = lab.argvs("ssh");
    assert_eq!(ssh.len(), 3, "exec, exec, probe");
    for argv in &ssh {
        lab.assert_on_the_master_and_pinned(argv);
    }
}

#[test]
fn env_reaches_the_command_through_stdin_and_never_through_argv() {
    let lab = lab();
    let value = "placeholder-7Hq2 with 'quotes' and $dollars";
    let mut spec = streamed(&lab, r#"printf '%s|' "$TOKEN"; ls -A "$TMPDIR""#);
    spec.env = vec![("TOKEN".to_owned(), value.to_owned())];
    assert_eq!(lab.exec(&spec), Ok(ExecOutcome::Exited { code: 0 }));
    assert_eq!(fs::read_to_string(lab.log()).unwrap(), format!("{value}|"));
    for argv in lab.argvs("ssh").iter().chain(&lab.argvs("crabbox")) {
        assert!(argv.iter().all(|a| !a.contains("7Hq2")), "{argv:?}");
    }
}

#[test]
fn an_env_name_that_is_not_a_shell_name_is_refused_before_ssh() {
    let lab = lab();
    let mut spec = sh("true");
    spec.env = vec![("A;rm".to_owned(), "v".to_owned())];
    assert!(matches!(lab.exec(&spec), Err(DriverError::Failed { .. })));
    assert!(lab.argvs("ssh").is_empty());
}

#[test]
fn the_command_exit_code_survives_even_when_it_is_255() {
    let lab = lab();
    for code in [3, 255] {
        for spec in [
            sh(&format!("exit {code}")),
            streamed(&lab, &format!("exit {code}")),
        ] {
            assert_eq!(lab.exec(&spec), Ok(ExecOutcome::Exited { code }));
        }
    }
}

#[test]
fn a_lost_connection_is_a_transport_error() {
    let lab = lab();
    assert_eq!(lab.exec(&sh("true")), Ok(ExecOutcome::Exited { code: 0 }));
    fs::write(lab.bin().join("drop"), "").unwrap();
    for spec in [sh("exit 255"), streamed(&lab, "exit 255")] {
        let got = lab.exec(&spec);
        assert!(matches!(got, Err(DriverError::Transport { .. })), "{got:?}");
    }
}

#[test]
fn streamed_output_arrives_in_order_without_the_wrapper_lines() {
    let lab = lab();
    let spec = streamed(&lab, "echo a; sleep 0.2; echo b >&2; sleep 0.2; echo c");
    assert_eq!(lab.exec(&spec), Ok(ExecOutcome::Exited { code: 0 }));
    assert_eq!(fs::read_to_string(lab.log()).unwrap(), "a\nb\nc\n");
}

#[test]
fn the_cwd_is_made_and_entered() {
    let lab = lab();
    let dir = lab.dir.path().join("work/repo");
    let mut spec = streamed(&lab, "pwd");
    spec.cwd = Some(BoxPath::restore(dir.to_str().unwrap()).unwrap());
    assert_eq!(lab.exec(&spec), Ok(ExecOutcome::Exited { code: 0 }));
    let printed = fs::read_to_string(lab.log()).unwrap();
    assert_eq!(
        Path::new(printed.trim_end()),
        fs::canonicalize(&dir).unwrap()
    );
}

#[test]
fn a_batch_stops_after_a_failing_run() {
    let lab = lab();
    let marker = lab.dir.path().join("ran");
    let steps = [
        BoxStep::Run("echo one; exit 4".to_owned()),
        BoxStep::Run(format!("touch {}", marker.display())),
    ];
    let done = lab
        .cb
        .batch(&handle(), &[], &steps, Deadline(Duration::from_secs(20)));
    assert_eq!(
        done,
        [Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 4 },
            stdout: "one\n".to_owned(),
        })]
    );
    assert!(!marker.exists());
}

#[test]
fn a_wrapper_failure_before_the_command_is_not_its_exit() {
    let err = ProcError::NonZero {
        code: 1,
        stdout: String::new(),
        stderr_tail: "moor: the command env did not load\n".to_owned(),
    };
    let lab = lab();
    let got = super::outcome(&lab.cb, &handle(), Err(err), "gate-0-exit");
    assert!(
        matches!(&got, Err(DriverError::Failed { summary }) if summary.contains("did not load")),
        "{got:?}"
    );
}
