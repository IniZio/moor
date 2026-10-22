//! Detached runs over a fake ssh that runs the remote command on this host.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::driver::{
    Cancelled, Driver, DriverError, ExecSpec, FollowEnd, Followed, Launched, RunEnd, RunOutcome,
};
use moor_core::seam::proc::Deadline;

use crate::adapters::crabbox::tests::fake_ssh::{handle, lab, Lab};
use crate::testing::driver::{eventually, live_in_group, reaped_pid};

const SECRET: &str = "placeholder-9Zx4 with 'quotes' and $dollars";

impl Lab {
    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn launch(&self, run: i64, script: &str) -> Result<Launched, DriverError> {
        let spec = ExecSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            env: vec![("TOKEN".to_owned(), SECRET.to_owned())],
            cwd: Some(BoxPath::restore(self.work().to_str().unwrap()).unwrap()),
            output: None,
        };
        self.cb.launch_run(&handle(), run, &spec)
    }

    fn follow(&self, run: i64, from: u64) -> (Result<Followed, DriverError>, String) {
        let mut out = Vec::new();
        let deadline = Deadline(Duration::from_secs(20));
        let r = self.cb.follow_run(&handle(), run, from, &mut out, deadline);
        (r, String::from_utf8(out).unwrap())
    }

    fn end(&self, run: i64) -> Result<RunEnd, DriverError> {
        let work = BoxPath::restore(self.work().to_str().unwrap()).unwrap();
        self.cb.end_run(&handle(), run, Some(&work))
    }

    /// Every ssh call rode the master with pinning, no env value reached an argv, and crabbox
    /// was only asked for the ssh target.
    fn assert_ssh_only(&self) {
        let ssh = self.argvs("ssh");
        assert!(!ssh.is_empty());
        for argv in &ssh {
            self.assert_on_the_master_and_pinned(argv);
            assert!(
                argv.iter().all(|a| !a.contains("placeholder-9Zx4")),
                "{argv:?}"
            );
        }
        for argv in self.argvs("crabbox") {
            assert_eq!(argv.first().map(String::as_str), Some("ssh"), "{argv:?}");
        }
    }
}

fn exited(code: u8, cancelled: bool) -> RunEnd {
    RunEnd {
        outcome: RunOutcome::Exited(code),
        cancelled,
        moved: false,
    }
}

#[test]
fn a_launched_run_gets_its_env_from_stdin_and_cwd_and_follows_to_its_exit() {
    let lab = lab();
    let launched = lab
        .launch(1, r#"printf '%s|' "$TOKEN"; pwd; exit 7"#)
        .unwrap();
    assert!(
        matches!(launched, Launched::Started { pid } if pid > 1),
        "{launched:?}"
    );
    let (followed, text) = lab.follow(1, 0);
    let want = format!("{SECRET}|{}\n", lab.work().display());
    assert_eq!(text, want);
    let offset = u64::try_from(want.len()).unwrap();
    assert_eq!(
        followed,
        Ok(Followed {
            offset,
            end: FollowEnd::Exited
        })
    );
    assert_eq!(lab.end(1), Ok(exited(7, false)));
    lab.assert_ssh_only();
    let launch = lab.argvs("ssh").into_iter().next().unwrap();
    let remote = launch.last().unwrap();
    assert!(
        remote.contains(r#"cat >"$1" && . "$1""#),
        "not in the wrapper: {remote}"
    );
}

#[test]
fn a_run_id_launches_once() {
    let lab = lab();
    assert!(matches!(
        lab.launch(2, "true"),
        Ok(Launched::Started { .. })
    ));
    assert_eq!(lab.launch(2, "true"), Ok(Launched::AlreadyLaunched));
}

#[test]
fn a_follow_cut_mid_run_returns_the_offset_it_reached_and_resumes_from_it() {
    let lab = lab();
    lab.launch(3, "printf 0123456789; sleep 1; printf abc")
        .unwrap();
    fs::write(lab.bin().join("cut"), "4").unwrap();
    let (followed, text) = lab.follow(3, 0);
    assert_eq!(
        followed,
        Ok(Followed {
            offset: 4,
            end: FollowEnd::Cut
        })
    );
    assert_eq!(text, "0123");
    fs::remove_file(lab.bin().join("cut")).unwrap();
    let (followed, text) = lab.follow(3, 4);
    assert_eq!(text, "456789abc");
    assert_eq!(
        followed,
        Ok(Followed {
            offset: 13,
            end: FollowEnd::Exited
        })
    );
    lab.assert_ssh_only();
}

#[test]
fn cancel_stops_every_process_of_the_run_and_marks_it_cancelled() {
    let lab = lab();
    let Launched::Started { pid } = lab.launch(4, "sleep 30 & sleep 30").unwrap() else {
        unreachable!("a fresh run id starts");
    };
    assert!(
        eventually(|| live_in_group(pid) > 2),
        "leader and two sleeps"
    );
    assert_eq!(lab.cb.cancel_run(&handle(), 4, 2), Ok(Cancelled::Signalled));
    assert!(
        eventually(|| live_in_group(pid) == 0),
        "the group outlived cancel"
    );
    assert!(lab.end(4).unwrap().cancelled);
    assert_eq!(lab.cb.cancel_run(&handle(), 5, 2), Ok(Cancelled::NoProcess));
    lab.assert_ssh_only();
}

#[test]
fn a_run_whose_process_is_gone_with_no_exit_follows_to_lost() {
    let lab = lab();
    let dir = lab.bin().join("home/.moor/runs/7");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("pid"), format!("{}\n", reaped_pid())).unwrap();
    fs::write(dir.join("log"), "partial").unwrap();
    fs::create_dir_all(lab.work()).unwrap();
    let (followed, text) = lab.follow(7, 0);
    assert_eq!(text, "partial");
    assert_eq!(
        followed,
        Ok(Followed {
            offset: 7,
            end: FollowEnd::Lost
        })
    );
    assert_eq!(lab.end(7).map(|e| e.outcome), Ok(RunOutcome::Lost));
}

#[test]
fn end_looks_for_the_moved_marker_in_the_workdir_it_is_given() {
    let lab = lab();
    lab.launch(8, "mkdir -p .moor && : > .moor/record-moved")
        .unwrap();
    assert!(eventually(|| lab
        .end(8)
        .is_ok_and(|e| e.outcome == RunOutcome::Exited(0))));
    assert!(lab.end(8).unwrap().moved, "read in the workdir");
    let in_login = lab.cb.end_run(&handle(), 8, None).unwrap();
    assert!(!in_login.moved, "read in the login dir");
    let remotes: Vec<String> = lab
        .argvs("ssh")
        .into_iter()
        .filter_map(|a| a.last().cloned())
        .filter(|r| r.contains("cancelled=%s"))
        .collect();
    let cd = format!(
        "sh -c 'cd -- '\\''{}'\\'' || exit 1\n",
        lab.work().display()
    );
    assert!(
        remotes.first().is_some_and(|r| r.starts_with(&cd)),
        "{remotes:?}"
    );
    assert!(
        remotes.last().is_some_and(|r| !r.contains("cd -- ")),
        "{remotes:?}"
    );
}

#[test]
fn a_dropped_connection_is_a_transport_error_for_end_and_cancel_and_a_cut_for_follow() {
    let lab = lab();
    fs::write(lab.bin().join("drop"), "").unwrap();
    assert!(matches!(lab.end(6), Err(DriverError::Transport { .. })));
    assert!(matches!(
        lab.cb.cancel_run(&handle(), 6, 2),
        Err(DriverError::Transport { .. })
    ));
    let (followed, _) = lab.follow(6, 9);
    assert_eq!(
        followed,
        Ok(Followed {
            offset: 9,
            end: FollowEnd::Cut
        })
    );
}
