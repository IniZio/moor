//! Target lookups without crabbox's ssh probes, and the lease touches of a long Box command.

use std::fs;
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;
use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;

use super::fake_ssh::{driver, handle, lab, tools, Lab};
use crate::adapters::crabbox::peek::lookup;

/// How often the lease is touched in the touch tests.
const EVERY: Duration = Duration::from_millis(50);

impl Lab {
    /// Makes the fake crabbox list `--no-probe` in `ssh --help`.
    fn knows_no_probe(&self) {
        fs::write(self.bin().join("no-probe"), "").unwrap();
    }

    fn helps(&self) -> usize {
        fs::read_dir(self.bin())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("help.")
            })
            .count()
    }
}

fn no_probe(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--no-probe")
}

#[test]
fn a_crabbox_that_knows_no_probe_is_looked_up_without_probing_and_asked_once() {
    let lab = lab();
    lab.knows_no_probe();
    lookup(&lab.cb, &handle()).unwrap();
    lookup(&lab.cb, &handle()).unwrap();
    let lookups = lab.argvs("crabbox");
    assert_eq!(lookups.len(), 2);
    for argv in &lookups {
        assert_eq!(argv.first().unwrap(), "ssh");
        assert!(no_probe(argv), "{argv:?}");
    }
    assert_eq!(lab.helps(), 1, "ssh --help is asked once per driver");
}

#[test]
fn drivers_wired_with_one_answer_ask_crabbox_once() {
    let lab = lab();
    lab.knows_no_probe();
    let known = Arc::new(OnceLock::new());
    for _ in 0..3 {
        let cb = driver(lab.dir.path(), &tools(&lab.bin())).sharing_no_probe(Arc::clone(&known));
        lookup(&cb, &handle()).unwrap();
    }
    assert_eq!(lab.helps(), 1);
    assert!(lab.argvs("crabbox").iter().all(|a| no_probe(a)));
}

#[test]
fn a_crabbox_without_no_probe_is_looked_up_as_before() {
    let lab = lab();
    let words = lookup(&lab.cb, &handle()).unwrap();
    assert!(words.iter().any(|w| w == "StrictHostKeyChecking=yes"));
    let lookups = lab.argvs("crabbox");
    assert_eq!(lookups.len(), 1);
    assert!(!lookups.iter().any(|a| no_probe(a)), "{lookups:?}");
    assert_eq!(lab.helps(), 1);
}

#[test]
fn a_failed_help_means_no_no_probe_and_the_next_start_asks_again() {
    let lab = lab();
    lab.knows_no_probe();
    fs::write(lab.bin().join("help-fails"), "").unwrap();
    lookup(&lab.cb, &handle()).unwrap();
    fs::remove_file(lab.bin().join("help-fails")).unwrap();
    let restarted = driver(lab.dir.path(), &tools(&lab.bin())).sharing_no_probe(Arc::default());
    lookup(&restarted, &handle()).unwrap();
    let lookups = lab.argvs("crabbox");
    let [failed, asked_again] = lookups.as_slice() else {
        unreachable!("{lookups:?}");
    };
    assert!(
        !no_probe(failed),
        "a failed help is not read as --no-probe: {failed:?}"
    );
    assert!(no_probe(asked_again), "{asked_again:?}");
    assert_eq!(lab.helps(), 2);
}

#[test]
fn a_running_command_touches_its_lease_every_ten_minutes() {
    // crabbox reaps a lease idle for its default 30 min, so a touch every 10 min leaves two
    // spare before a busy Box is lost.
    assert_eq!(lab().cb.touch_every, Duration::from_mins(10));
}

#[test]
fn a_shared_call_keeps_its_master_up_for_ten_minutes() {
    let lab = lab();
    let path = BoxPath::restore(&lab.dir.path().join("absent").display().to_string()).unwrap();
    assert_eq!(lab.cb.peek_file(&handle(), &path), Ok(None));
    let ssh = lab.argvs("ssh");
    let persist = ssh
        .first()
        .and_then(|argv| argv.iter().find(|a| a.starts_with("ControlPersist=")));
    assert_eq!(
        persist.map(String::as_str),
        Some("ControlPersist=600"),
        "{ssh:?}"
    );
}

/// A lab whose driver touches a running command's lease every [`EVERY`].
fn touching_lab() -> Arc<Lab> {
    let Lab { dir, cb } = lab();
    let lab = Lab {
        dir,
        cb: cb.with_touch_every(EVERY),
    };
    lab.knows_no_probe();
    Arc::new(lab)
}

/// Waits about 20 s at most for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let reached = (0..2000).any(|_| {
        thread::sleep(Duration::from_millis(10));
        done()
    });
    assert!(reached, "timed out waiting for {what}");
}

#[test]
fn a_long_command_touches_the_lease_while_it_runs_and_stops_when_it_ends() {
    let lab = touching_lab();
    let release = lab.dir.path().join("release");
    let spec = ExecSpec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!(
                "while ! test -f '{}'; do sleep 0.02; done",
                release.display()
            ),
        ],
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let (sent, outcome) = mpsc::channel();
    let running = Arc::clone(&lab);
    thread::spawn(move || {
        let out = running
            .cb
            .exec(&handle(), &spec, Deadline(Duration::from_secs(60)));
        let _ = sent.send(out);
    });
    wait_for("three touches", || lab.argvs("heartbeat").len() >= 3);
    fs::write(&release, "").unwrap();
    let ended = outcome
        .recv_timeout(Duration::from_secs(20))
        .expect("the exec returns once its command ends");
    assert_eq!(ended, Ok(ExecOutcome::Exited { code: 0 }));
    let touched = lab.argvs("heartbeat");
    for argv in &touched {
        assert_eq!(
            argv,
            &[
                "heartbeat",
                "--provider",
                "external",
                "--id",
                "moor-7-ab12cd"
            ]
        );
    }
    thread::sleep(EVERY * 10);
    assert_eq!(
        lab.argvs("heartbeat").len(),
        touched.len(),
        "no touch after the command ended"
    );
}
