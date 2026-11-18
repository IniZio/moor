//! The controller's machine driver against a machine that provides fake Boxes, through a real
//! runner: open, exec in both places, files over exec, sleep, wake and close.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor::{Machines, Reach, SentEnvs};
use moor_client::link::reach::Runner;
use moor_core::box_key::{ControllerId, PoolSeat};
use moor_core::cache::CachePlan;
use moor_core::name::{BoxId, BoxName, BoxPath, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxStep, Driver, DriverError, ExecOutcome, ExecSpec, Place, Released,
    SleepOutcome, StepDone,
};
use moor_core::seam::proc::{Deadline, Output};

use crate::broker::Host;
use crate::rig::MACHINE_ID;

/// The runner the host's agent is linked to, whichever machine is asked for.
struct Local(Runner);

impl Reach for Local {
    fn runner(&self, _machine_id: i64) -> Result<Runner, String> {
        Ok(self.0.clone())
    }
}

fn machines(host: &Host) -> Machines {
    Machines::new(
        Arc::new(Local(host.runner())),
        PathBuf::from("/usr/bin/moor"),
    )
}

fn request(id: i64) -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new("boxes"),
        box_id: BoxId::new(id),
        name: BoxName::parse_allow_bare("pilot").unwrap(),
        class: None,
        nonce: "abc123".to_owned(),
        cache: CachePlan::default(),
        pool_machine: Some(PoolSeat {
            machine: MACHINE_ID,
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
            inc: 1,
        }),
    }
}

fn opened(m: &Machines, id: i64) -> Acquired {
    m.acquire(&request(id)).unwrap()
}

/// Runs `script` in `place`, its output into a file; how it ended and what it wrote.
fn run(
    m: &Machines,
    host: &Host,
    got: &Acquired,
    place: Place,
    script: &str,
) -> (ExecOutcome, String) {
    let log = host.rig.dir().join("out.log");
    let spec = ExecSpec::new(place, vec!["sh".into(), "-c".into(), script.to_owned()])
        .output(Some(Output::Log(log.clone())));
    let outcome = m
        .exec(&got.handle, &spec, Deadline(Duration::from_secs(20)))
        .unwrap();
    (outcome, std::fs::read_to_string(log).unwrap_or_default())
}

#[test]
fn open_makes_the_box_and_names_its_handle_and_workdir() {
    let host = Host::boxes();
    let m = machines(&host);
    let req = request(41);
    let planned = m.planned_handle(&req);
    assert_eq!(planned.as_str(), "m7/moor-1a2b3c4d-41");
    let mut notes = Vec::new();
    let got = m
        .acquire_noting(&req, &mut |n| notes.push(n.to_owned()))
        .unwrap();
    assert_eq!(got.handle, planned);
    assert!(
        got.workdir.as_str().ends_with("/moor-1a2b3c4d-41/workdir"),
        "{got:?}"
    );
    assert_eq!(notes, ["making the fake Box"]);
    assert_eq!(
        opened(&m, 41).handle,
        planned,
        "open again finds the same Box"
    );
    assert_eq!(m.workdir(&got.handle).unwrap(), got.workdir);
}

#[test]
fn exec_runs_in_both_places_and_reports_each_exit_code() {
    let host = Host::boxes();
    let m = machines(&host);
    let got = opened(&m, 42);
    for place in [Place::Agent, Place::Work] {
        let (outcome, out) = run(&m, &host, &got, place, "echo $MOOR_PLACE; pwd");
        assert_eq!(outcome, ExecOutcome::Exited { code: 0 });
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(place.as_str()));
        assert_eq!(lines.next(), Some(got.workdir.as_str()));
        let (outcome, _) = run(&m, &host, &got, place, "exit 7");
        assert_eq!(outcome, ExecOutcome::Exited { code: 7 });
    }
}

#[test]
fn a_deadline_ends_a_command_that_outlives_it() {
    let host = Host::boxes();
    let m = machines(&host);
    let got = opened(&m, 43);
    let spec = ExecSpec::new(Place::Work, vec!["sleep".into(), "30".into()]);
    let started = std::time::Instant::now();
    let err = m
        .exec(&got.handle, &spec, Deadline(Duration::from_millis(500)))
        .unwrap_err();
    assert_eq!(err, DriverError::Timeout(Duration::from_millis(500)));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn files_and_batches_run_over_exec_in_the_place_named() {
    let host = Host::boxes();
    let m = machines(&host);
    let got = opened(&m, 44);
    let file = BoxPath::under(got.workdir.as_str(), "notes/hello.txt").unwrap();
    m.put_file(&got.handle, Place::Work, &file, b"hi there\n", 0o640)
        .unwrap();
    let read = m.peek_file(&got.handle, Place::Work, &file).unwrap();
    assert_eq!(read.as_deref(), Some("hi there\n"));
    let gone = BoxPath::under(got.workdir.as_str(), "notes/none.txt").unwrap();
    assert_eq!(m.peek_file(&got.handle, Place::Agent, &gone).unwrap(), None);
    let steps = [
        BoxStep::Peek(file.clone()),
        BoxStep::Run("echo from-$MOOR_PLACE".to_owned()),
        BoxStep::Run("exit 3".to_owned()),
        BoxStep::Run("echo not-reached".to_owned()),
    ];
    let done = m.batch(
        &got.handle,
        &[],
        Place::Agent,
        &steps,
        Deadline(Duration::from_secs(20)),
    );
    assert_eq!(
        done.len(),
        3,
        "the batch stops after the step that failed: {done:?}"
    );
    assert_eq!(done[0], Ok(StepDone::Peeked(Some("hi there\n".to_owned()))));
    assert_eq!(
        done[1],
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout: "from-agent\n".to_owned()
        })
    );
    assert!(matches!(
        done[2],
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 3 },
            ..
        })
    ));
}

#[test]
fn a_file_is_copied_out_of_the_box() {
    let host = Host::boxes();
    let m = machines(&host);
    let got = opened(&m, 45);
    let file = BoxPath::under(got.workdir.as_str(), "out/report.txt").unwrap();
    m.put_file(&got.handle, Place::Work, &file, b"report\n", 0o600)
        .unwrap();
    let to = host.rig.dir().join("copied.txt");
    m.copy_out(&got.handle, Place::Work, &file, &to).unwrap();
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "report\n");
}

#[test]
fn sleep_wake_and_close_follow_the_box_and_close_is_repeatable() {
    let host = Host::boxes();
    let m = machines(&host);
    let got = opened(&m, 46);
    assert_eq!(m.sleep(&got.handle), Ok(SleepOutcome::Slept));
    let listed = m.list_handles(std::slice::from_ref(&got.handle)).unwrap();
    assert_eq!(listed.answered, std::slice::from_ref(&got.handle));
    assert!(listed.listed[0].asleep, "{listed:?}");
    m.wake(&got.handle).unwrap();
    let (outcome, _) = run(&m, &host, &got, Place::Work, "true");
    assert_eq!(outcome, ExecOutcome::Exited { code: 0 });
    assert_eq!(m.release(&got.handle), Ok(Released::Deleted));
    assert_eq!(m.release(&got.handle), Ok(Released::Deleted));
    let listed = m.list_handles(std::slice::from_ref(&got.handle)).unwrap();
    assert_eq!(listed.answered, std::slice::from_ref(&got.handle));
    assert!(listed.listed.is_empty(), "a closed Box is gone: {listed:?}");
    assert_eq!(m.wake(&got.handle), Err(DriverError::NotFound));
    let spec = ExecSpec::new(Place::Work, vec!["true".into()]);
    let err = m.exec(&got.handle, &spec, Deadline(Duration::from_secs(5)));
    assert_eq!(err, Err(DriverError::NotFound));
}

#[test]
fn a_machine_that_provides_boxes_links_saying_how() {
    let host = Host::boxes();
    let links = host.rig.link.links.lock().unwrap().clone().unwrap();
    let provides = links[0]["provides"].clone();
    assert_eq!(provides, "fake");
}

/// The runner the host's agent is linked to, once the machine's link is back after `down` asks.
struct Relinking {
    runner: Runner,
    down: AtomicU32,
}

impl Reach for Relinking {
    fn runner(&self, machine_id: i64) -> Result<Runner, String> {
        let left = self.down.load(Ordering::SeqCst);
        if left > 0 {
            self.down.store(left - 1, Ordering::SeqCst);
            return Err(format!("machine {machine_id} is not linked"));
        }
        Ok(self.runner.clone())
    }
}

/// Takes the machine's link down for the next `asks` calls.
fn drop_link(reach: &Relinking, asks: u32) {
    reach.down.store(asks, Ordering::SeqCst);
}

#[test]
fn calls_safe_to_repeat_wait_for_the_relink_and_a_plain_exec_does_not() {
    let host = Host::boxes();
    let reach = Arc::new(Relinking {
        runner: host.runner(),
        down: AtomicU32::new(0),
    });
    let m = Machines::new(reach.clone(), PathBuf::from("/usr/bin/moor"));
    let got = opened(&m, 47);
    let deadline = Deadline(Duration::from_secs(10));
    drop_link(&reach, 2);
    let marked = ExecSpec::new(Place::Agent, vec!["true".into()]).repeatable();
    assert_eq!(
        m.exec(&got.handle, &marked, deadline),
        Ok(ExecOutcome::Exited { code: 0 })
    );
    drop_link(&reach, 2);
    let steps = [BoxStep::Repeatable("echo probed".to_owned())];
    let done = m.batch(&got.handle, &[], Place::Agent, &steps, deadline);
    assert!(
        matches!(&done[..], [Ok(StepDone::Ran { stdout, .. })] if stdout.trim() == "probed"),
        "{done:?}"
    );
    drop_link(&reach, 2);
    assert_eq!(m.sleep(&got.handle), Ok(SleepOutcome::Slept));
    drop_link(&reach, 2);
    m.wake(&got.handle).unwrap();
    drop_link(&reach, 1);
    let plain = ExecSpec::new(Place::Work, vec!["true".into()]);
    let err = m.exec(&got.handle, &plain, deadline);
    assert!(
        matches!(&err, Err(DriverError::Unavailable(why)) if why.contains("not linked")),
        "{err:?}"
    );
    drop_link(&reach, 1);
    let steps = [BoxStep::Run("true".to_owned())];
    let done = m.batch(&got.handle, &[], Place::Agent, &steps, deadline);
    assert!(
        matches!(&done[..], [Err(DriverError::Unavailable(_))]),
        "{done:?}"
    );
    assert_eq!(m.release(&got.handle), Ok(Released::Deleted));
}

#[test]
fn a_box_is_given_an_env_once_across_the_requests_of_one_controller() {
    let host = Host::boxes();
    let reach = Arc::new(Relinking {
        runner: host.runner(),
        down: AtomicU32::new(0),
    });
    let sent = Arc::new(SentEnvs::default());
    let request = || {
        Machines::new(reach.clone(), PathBuf::from("/usr/bin/moor"))
            .sharing_sent_env(Arc::clone(&sent))
    };
    let got = opened(&request(), 48);
    let proxy = vec![("HTTPS_PROXY".to_owned(), "http://127.0.0.1:3128".to_owned())];
    request().set_env(&got.handle, &proxy, Some("ca")).unwrap();
    drop_link(&reach, u32::MAX);
    assert_eq!(request().set_env(&got.handle, &proxy, Some("ca")), Ok(()));
    drop_link(&reach, 0);
    let moved = vec![("HTTPS_PROXY".to_owned(), "http://127.0.0.1:3129".to_owned())];
    request().set_env(&got.handle, &moved, Some("ca")).unwrap();
    assert_eq!(machines(&host).release(&got.handle), Ok(Released::Deleted));
    let again = opened(&request(), 48);
    request()
        .set_env(&again.handle, &moved, Some("ca"))
        .unwrap();
    let (_, out) = run(&request(), &host, &again, Place::Work, "echo $HTTPS_PROXY");
    assert_eq!(out.trim(), "http://127.0.0.1:3129");
    assert_eq!(request().release(&again.handle), Ok(Released::Deleted));
}
