//! Tests for per-Box admission through one owner.

#![allow(clippy::panic, reason = "an op that panics is the case under test")]

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, Scope, ScopedJoinHandle};
use std::time::{Duration, Instant};

use moor_core::name::BoxId;
use moor_core::seam::clock::Clock;
use moor_core::ErrorKind;

use moor_core::seam::clock::Ts;

use super::{Cmd, Owners, INTERRUPT_WAIT};
use crate::adapters::clock::SystemClock;
use crate::env::Cancel;
use crate::error::CommandError;
use crate::testing::clock::FakeClock;

const B: BoxId = BoxId::new(7);

fn soon() -> Instant {
    SystemClock::real().instant() + Duration::from_secs(10)
}

fn owners() -> Owners {
    Owners::new(Arc::new(SystemClock::real()))
}

/// Waits, briefly, for `ready` to hold.
fn until(ready: impl Fn() -> bool) {
    let deadline = soon();
    while !ready() {
        assert!(
            SystemClock::real().instant() < deadline,
            "condition never held"
        );
        thread::yield_now();
    }
}

/// Holds `B`'s turn for `cmd` on a scoped thread until the returned sender is dropped or sent to.
fn hold<'s>(
    s: &'s Scope<'s, '_>,
    owners: &'s Owners,
    cmd: Cmd,
    cancel: Cancel,
) -> (mpsc::Sender<()>, ScopedJoinHandle<'s, ()>) {
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx): (_, Receiver<()>) = mpsc::channel();
    let join = s.spawn(move || {
        owners
            .call(B, cmd, &cancel, soon(), |_| {
                held_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok(())
            })
            .unwrap();
    });
    held_rx.recv().unwrap();
    (release_tx, join)
}

fn refused(err: &CommandError) -> String {
    assert_eq!(err.kind, ErrorKind::Refused, "{}", err.error);
    err.error.to_string()
}

#[test]
fn queued_ops_run_in_arrival_order() {
    let owners = owners();
    for _ in 0..1000 {
        let ran = Mutex::new(Vec::new());
        thread::scope(|s| {
            let (release, _) = hold(s, &owners, Cmd::Wake, Cancel::default());
            for (i, cmd) in [Cmd::Steer, Cmd::Sleep, Cmd::Close { force: false }]
                .into_iter()
                .enumerate()
            {
                let (owners, ran) = (&owners, &ran);
                s.spawn(move || {
                    owners
                        .call(B, cmd, &Cancel::default(), soon(), |_| {
                            ran.lock().unwrap().push(cmd);
                            Ok(())
                        })
                        .unwrap();
                });
                until(|| owners.waiting(B) == i + 1);
            }
            drop(release);
        });
        assert_eq!(
            ran.into_inner().unwrap(),
            [Cmd::Steer, Cmd::Sleep, Cmd::Close { force: false }]
        );
    }
}

#[test]
fn try_call_skips_a_busy_box() {
    let owners = owners();
    thread::scope(|s| {
        let (release, join) = hold(s, &owners, Cmd::Steer, Cancel::default());
        assert_eq!(
            owners.try_call(B, Cmd::Settle, &Cancel::default(), |_| 1),
            None
        );
        assert_eq!(
            owners.try_call(BoxId::new(8), Cmd::Settle, &Cancel::default(), |_| 2),
            Some(2)
        );
        drop(release);
        join.join().unwrap();
    });
    assert_eq!(
        owners.try_call(B, Cmd::Settle, &Cancel::default(), |_| 3),
        Some(3)
    );
}

#[test]
fn setup_refuses_user_ops_and_a_forced_close_preempts_it() {
    let owners = owners();
    let ran = Mutex::new(Vec::new());
    thread::scope(|s| {
        let (held_tx, held_rx) = mpsc::channel();
        let (owners, ran) = (&owners, &ran);
        let start = s.spawn(move || {
            let cancel = Cancel::default();
            owners
                .call(B, Cmd::Start, &cancel, soon(), |_| {
                    held_tx.send(()).unwrap();
                    until(|| cancel.is_cancelled());
                    ran.lock().unwrap().push("start stopped");
                    Ok(())
                })
                .unwrap();
        });
        held_rx.recv().unwrap();
        for cmd in [Cmd::Sleep, Cmd::Steer, Cmd::Close { force: false }] {
            let err = owners
                .call(B, cmd, &Cancel::default(), soon(), |_| Ok(()))
                .unwrap_err();
            assert_eq!(refused(&err), "box busy: setup in progress");
        }
        owners
            .call(
                B,
                Cmd::Close { force: true },
                &Cancel::default(),
                soon(),
                |_| {
                    ran.lock().unwrap().push("closed");
                    Ok(())
                },
            )
            .unwrap();
        start.join().unwrap();
    });
    assert_eq!(ran.into_inner().unwrap(), ["start stopped", "closed"]);
}

#[test]
fn a_panicking_op_releases_the_turn_to_the_next_waiter() {
    let owners = owners();
    thread::scope(|s| {
        let (held_tx, held_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let owners = &owners;
        let panicker = s.spawn(move || {
            owners.call(
                B,
                Cmd::Wake,
                &Cancel::default(),
                soon(),
                |_| -> Result<(), _> {
                    held_tx.send(()).unwrap();
                    let _ = go_rx.recv();
                    panic!("op failed");
                },
            )
        });
        held_rx.recv().unwrap();
        let waiter = s.spawn(|| owners.call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(5)));
        until(|| owners.waiting(B) == 1);
        drop(go_tx);
        assert!(panicker.join().is_err());
        assert_eq!(waiter.join().unwrap().unwrap(), 5);
    });
}

#[test]
fn an_idle_owner_is_dropped() {
    let owners = owners();
    owners
        .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| {
            assert_eq!(owners.owned(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(owners.owned(), 0);
    let _ = owners.try_call(B, Cmd::Settle, &Cancel::default(), |_| ());
    assert_eq!(owners.owned(), 0);
}

#[test]
fn past_sixteen_waiters_a_call_is_refused() {
    let owners = owners();
    let ran = Mutex::new(0);
    thread::scope(|s| {
        let (release, _) = hold(s, &owners, Cmd::Wake, Cancel::default());
        for i in 0..16 {
            s.spawn(|| {
                owners
                    .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| {
                        *ran.lock().unwrap() += 1;
                        Ok(())
                    })
                    .unwrap();
            });
            until(|| owners.waiting(B) == i + 1);
        }
        let err = owners
            .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(()))
            .unwrap_err();
        assert_eq!(refused(&err), "box busy: 16 queued");
        drop(release);
    });
    assert_eq!(ran.into_inner().unwrap(), 16);
}

#[test]
fn drain_wakes_waiters_with_a_restarting_error() {
    let owners = owners();
    thread::scope(|s| {
        let (release, join) = hold(s, &owners, Cmd::Wake, Cancel::default());
        let waiters: Vec<_> = (0..2)
            .map(|i| {
                let w =
                    s.spawn(|| owners.call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(())));
                until(|| owners.waiting(B) == i + 1);
                w
            })
            .collect();
        owners.drain();
        for w in waiters {
            assert_eq!(
                refused(&w.join().unwrap().unwrap_err()),
                "controller restarting"
            );
        }
        let err = owners
            .call(
                BoxId::new(8),
                Cmd::Steer,
                &Cancel::default(),
                soon(),
                |_| Ok(()),
            )
            .unwrap_err();
        assert_eq!(refused(&err), "controller restarting");
        drop(release);
        join.join().unwrap();
    });
}

#[test]
fn calling_the_held_box_again_errors_instead_of_deadlocking() {
    let owners = owners();
    let inner = owners
        .call(
            B,
            Cmd::Close { force: false },
            &Cancel::default(),
            soon(),
            |_| Ok(owners.call(B, Cmd::Promote, &Cancel::default(), soon(), |_| Ok(()))),
        )
        .unwrap();
    assert_eq!(inner.unwrap_err().kind, ErrorKind::Internal);
}

#[test]
fn a_run_in_flight_admits_steer_and_refuses_sleep_close_and_another_run() {
    let owners = owners();
    let run = owners
        .call(B, Cmd::Run, &Cancel::default(), soon(), |turn| {
            Ok(turn.begin_run())
        })
        .unwrap();
    owners
        .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(()))
        .unwrap();
    for cmd in [Cmd::Sleep, Cmd::Close { force: false }, Cmd::Run] {
        let err = owners
            .call(B, cmd, &Cancel::default(), soon(), |_| Ok(()))
            .unwrap_err();
        assert_eq!(refused(&err), "box busy: run in progress");
    }
    assert_eq!(
        owners.try_call(B, Cmd::Settle, &Cancel::default(), |_| ()),
        None
    );
    drop(run);
    owners
        .call(B, Cmd::Sleep, &Cancel::default(), soon(), |_| Ok(()))
        .unwrap();
}

#[test]
fn a_forced_close_is_refused_when_setup_ignores_its_cancel() {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let owners = Owners::new(clock.clone());
    let deadline = clock.instant() + INTERRUPT_WAIT * 4;
    let setup = Cancel::default();
    thread::scope(|s| {
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (owners, setup) = (&owners, &setup);
        let start = s.spawn(move || {
            owners.call(B, Cmd::Start, setup, deadline, |_| {
                held_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok(())
            })
        });
        held_rx.recv().unwrap();
        let close = s.spawn(move || {
            owners.call(
                B,
                Cmd::Close { force: true },
                &Cancel::default(),
                deadline,
                |_| Ok(()),
            )
        });
        until(|| setup.is_cancelled());
        clock.sleep(INTERRUPT_WAIT);
        let err = close.join().unwrap().unwrap_err();
        assert_eq!(refused(&err), "box busy: start did not stop after cancel");
        drop(release_tx);
        start.join().unwrap().unwrap();
    });
    owners
        .call(
            B,
            Cmd::Close { force: true },
            &Cancel::default(),
            deadline,
            |_| Ok(()),
        )
        .unwrap();
}

#[test]
fn a_helper_thread_acts_under_a_shared_turn_without_queueing() {
    let owners = owners();
    thread::scope(|s| {
        let (held_tx, held_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let owners = &owners;
        let parent = s.spawn(move || {
            owners.call(B, Cmd::Relaunch, &Cancel::default(), soon(), |turn| {
                let share = turn.share();
                held_tx.send(()).unwrap();
                let _ = go_rx.recv();
                let helper = thread::scope(|h| h.spawn(move || share.check()).join().unwrap());
                Ok(helper.unwrap())
            })
        });
        held_rx.recv().unwrap();
        let waiter = s.spawn(|| owners.call(B, Cmd::Wake, &Cancel::default(), soon(), |_| Ok(())));
        until(|| owners.waiting(B) == 1);
        drop(go_tx);
        assert_eq!(parent.join().unwrap().unwrap(), B);
        waiter.join().unwrap().unwrap();
    });
}

#[test]
fn a_share_is_rejected_once_its_turn_ends() {
    let owners = owners();
    let share = owners
        .call(B, Cmd::Steer, &Cancel::default(), soon(), |turn| {
            let share = turn.share();
            assert_eq!(share.check().unwrap(), B);
            Ok(share)
        })
        .unwrap();
    assert_eq!(share.check().unwrap_err().kind, ErrorKind::Internal);
    owners
        .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| {
            assert_eq!(share.check().unwrap_err().kind, ErrorKind::Internal);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_waiter_that_times_out_leaves_the_queue_to_the_next() {
    let owners = owners();
    thread::scope(|s| {
        let (release, _) = hold(s, &owners, Cmd::Wake, Cancel::default());
        let short = SystemClock::real().instant() + Duration::from_millis(200);
        let owners = &owners;
        let first =
            s.spawn(move || owners.call(B, Cmd::Steer, &Cancel::default(), short, |_| Ok(1)));
        until(|| owners.waiting(B) == 1);
        let second = s.spawn(|| owners.call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(2)));
        until(|| owners.waiting(B) == 2);
        let err = first.join().unwrap().unwrap_err();
        assert_eq!(refused(&err), "box busy: wake in progress");
        drop(release);
        assert_eq!(second.join().unwrap().unwrap(), 2);
    });
}

#[test]
fn a_newcomer_racing_the_head_waits_behind_it() {
    let owners = owners();
    for _ in 0..1000 {
        let ran = Mutex::new(Vec::new());
        let race = Barrier::new(2);
        thread::scope(|s| {
            let (held_tx, held_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let (owners, ran, race) = (&owners, &ran, &race);
            s.spawn(move || {
                owners.call(B, Cmd::Wake, &Cancel::default(), soon(), |_| {
                    held_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                    race.wait();
                    Ok(())
                })
            });
            held_rx.recv().unwrap();
            let push = move |who| {
                move |_: &_| {
                    ran.lock().unwrap().push(who);
                    Ok(())
                }
            };
            s.spawn(move || owners.call(B, Cmd::Steer, &Cancel::default(), soon(), push("head")));
            until(|| owners.waiting(B) == 1);
            s.spawn(move || {
                race.wait();
                owners.call(B, Cmd::Steer, &Cancel::default(), soon(), push("newcomer"))
            });
            drop(release_tx);
        });
        assert_eq!(ran.into_inner().unwrap(), ["head", "newcomer"]);
    }
}

#[test]
fn a_poisoned_lock_is_recovered() {
    let owners = owners();
    let poisoned = thread::scope(|s| {
        s.spawn(|| {
            let _held = owners.state.lock();
            panic!("poison the owners' lock");
        })
        .join()
    });
    assert!(poisoned.is_err());
    owners
        .call(B, Cmd::Steer, &Cancel::default(), soon(), |_| Ok(()))
        .unwrap();
    assert_eq!(
        owners.try_call(B, Cmd::Settle, &Cancel::default(), |_| 1),
        Some(1)
    );
}

#[test]
fn a_run_hold_is_released_when_its_thread_panics() {
    let owners = owners();
    let run = owners
        .call(B, Cmd::Run, &Cancel::default(), soon(), |turn| {
            Ok(turn.begin_run())
        })
        .unwrap();
    let panicked = thread::scope(|s| {
        s.spawn(move || {
            let _run = run;
            panic!("exec failed");
        })
        .join()
    });
    assert!(panicked.is_err());
    owners
        .call(B, Cmd::Sleep, &Cancel::default(), soon(), |_| Ok(()))
        .unwrap();
}

#[test]
fn two_boxes_crossing_to_each_other_from_inside_their_turns_cannot_deadlock() {
    let owners = owners();
    let other = BoxId::new(8);
    let crossed = Mutex::new(Vec::new());
    let both_tried = std::sync::Barrier::new(2);
    thread::scope(|s| {
        let (owners, crossed, both_tried) = (&owners, &crossed, &both_tried);
        let (a_in, a_held) = mpsc::channel();
        let (b_in, b_held) = mpsc::channel();
        let cross = move |mine: BoxId,
                          theirs: BoxId,
                          held: mpsc::Sender<()>,
                          peer: Receiver<()>| {
            owners
                .call(mine, Cmd::Start, &Cancel::default(), soon(), |_| {
                    held.send(()).unwrap();
                    peer.recv().unwrap();
                    let waited =
                        owners.call(theirs, Cmd::Suspend, &Cancel::default(), soon(), |_| Ok(()));
                    assert_eq!(waited.unwrap_err().kind, ErrorKind::Internal);
                    let tried = owners.try_call(theirs, Cmd::Suspend, &Cancel::default(), |_| ());
                    crossed.lock().unwrap().push(tried);
                    both_tried.wait();
                    Ok(())
                })
                .unwrap();
        };
        let a = s.spawn(move || cross(B, other, a_in, b_held));
        let b = s.spawn(move || cross(other, B, b_in, a_held));
        a.join().unwrap();
        b.join().unwrap();
    });
    assert_eq!(crossed.into_inner().unwrap(), [None, None]);
}
