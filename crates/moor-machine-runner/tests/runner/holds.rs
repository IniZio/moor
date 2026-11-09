use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::rig::{caller, Rig};

/// A command that fails loudly if another holder of `name` is inside it at the same time.
fn alone(name: &str, secs: &str) -> String {
    format!(
        "test ! -e $HOME/{name}.busy || echo OVERLAP; touch $HOME/{name}.busy; \
         sleep {secs}; rm $HOME/{name}.busy; echo done"
    )
}

fn exec(command: &str, hold: &[&str]) -> Value {
    json!({"command": command, "hold": hold})
}

/// Runs each `(box id, machine, args)` exec 300 ms after the one before, all at once.
fn concurrently(
    rig: &Arc<Rig>,
    calls: Vec<(&'static str, &'static str, Value)>,
) -> Vec<(Result<Value, String>, Vec<String>, Duration)> {
    let ids = calls.iter().map(|(id, _, _)| id.parse::<i64>().unwrap());
    rig.open.lock().unwrap().extend(ids);
    let started = Instant::now();
    let handles: Vec<_> = calls
        .into_iter()
        .enumerate()
        .map(|(i, (id, machine, args))| {
            let rig = Arc::clone(rig);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300 * i as u64));
                let headers = caller(&rig, id, machine);
                let (result, progress) = rig.call_progress("exec", args, &headers);
                (result, progress, started.elapsed())
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn output(result: &Result<Value, String>) -> String {
    result.as_ref().unwrap()["output"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn one_build_slot_runs_two_calls_one_after_the_other() {
    let rig = Arc::new(Rig::with_resources(&["build=1"]));
    let runs = concurrently(
        &rig,
        vec![
            ("12", "minion", exec(&alone("build", "2"), &["build"])),
            ("7", "minion", exec(&alone("build", "1"), &["build"])),
        ],
    );
    for (result, _, _) in &runs {
        assert_eq!(output(result), "done\n", "{result:?}");
    }
    assert!(
        runs[1].2 >= Duration::from_secs(3),
        "second waited: {:?}",
        runs[1].2
    );
    assert!(runs[0].1.is_empty(), "{:?}", runs[0].1);
    assert!(
        runs[1]
            .1
            .iter()
            .any(|m| m == "waiting for build on minion: held by Box 12"),
        "{:?}",
        runs[1].1
    );
}

#[test]
fn two_sim_slots_run_two_at_once_and_a_third_waits() {
    let rig = Arc::new(Rig::with_resources(&["sim=2"]));
    rig.open.lock().unwrap().extend([7, 8, 9]);
    let until_released = |name: &str| {
        format!(
            "touch $HOME/{name}.started; until [ -e $HOME/released ]; do sleep 0.1; done; echo done"
        )
    };
    let start = |id: &'static str, name: &str| {
        let rig = Arc::clone(&rig);
        let args = exec(&until_released(name), &["sim"]);
        thread::spawn(move || {
            let headers = caller(&rig, id, "minion");
            rig.call_progress("exec", args, &headers)
        })
    };
    let first = start("7", "a");
    let second = start("8", "b");
    wait_for(&rig.remote.join("a.started"));
    wait_for(&rig.remote.join("b.started"));
    let third = start("9", "c");
    thread::sleep(Duration::from_secs(1));
    assert!(
        !rig.remote.join("c.started").exists(),
        "the third ran with both slots taken"
    );
    std::fs::write(rig.remote.join("released"), "").unwrap();
    let (third, third_progress) = third.join().unwrap();
    assert_eq!(output(&third), "done\n");
    for held in [first, second] {
        let (result, progress) = held.join().unwrap();
        assert_eq!(output(&result), "done\n");
        assert!(progress.is_empty(), "two ran at once: {progress:?}");
    }
    assert!(
        third_progress[0].starts_with("waiting for sim on minion: held by Box "),
        "{third_progress:?}"
    );
}

#[test]
fn a_hold_on_another_machine_does_not_block() {
    let rig = Arc::new(Rig::with_resources(&["build=1"]));
    rig.open.lock().unwrap().extend([7, 8]);
    let holder = {
        let rig = Arc::clone(&rig);
        thread::spawn(move || {
            let until_released =
                "touch $HOME/held; until [ -e $HOME/released ]; do sleep 0.1; done; echo done";
            let headers = caller(&rig, "7", "minion");
            rig.call_progress("exec", exec(until_released, &["build"]), &headers)
        })
    };
    wait_for(&rig.remote.join("held"));
    let headers = caller(&rig, "8", "other");
    let release =
        json!({"command": "touch $HOME/released", "hold": ["build"], "hold_wait_secs": 5});
    let (result, progress) = rig.call_progress("exec", release, &headers);
    result.unwrap();
    assert!(progress.is_empty(), "{progress:?}");
    let (held, _) = holder.join().unwrap();
    assert_eq!(output(&held), "done\n");
}

#[test]
fn a_wait_past_hold_wait_secs_returns_the_reason_and_runs_nothing() {
    let rig = Arc::new(Rig::with_resources(&["build=1"]));
    let late = json!({"command": "touch $HOME/late-ran", "hold": ["build"], "hold_wait_secs": 1});
    let runs = concurrently(
        &rig,
        vec![
            ("12", "minion", exec("sleep 4", &["build"])),
            ("7", "minion", late),
        ],
    );
    let err = runs[1].0.as_ref().unwrap_err();
    assert!(
        err.contains("waiting for build on minion: held by Box 12"),
        "{err}"
    );
    assert!(runs[1].2 < Duration::from_secs(3), "{:?}", runs[1].2);
    assert!(
        !rig.remote.join("late-ran").exists(),
        "the command did not run"
    );
    let err = rig
        .call(
            "exec",
            json!({"command": "true", "hold": ["build"], "hold_wait_secs": 3601}),
        )
        .unwrap_err();
    assert!(err.contains("3600"), "{err}");
}

#[test]
fn an_unknown_resource_is_refused_at_once() {
    let rig = Rig::with_resources(&["build=1"]);
    rig.call("sync", json!({})).unwrap();
    let err = rig.call("exec", exec("touch ran", &["gpu"])).unwrap_err();
    assert!(err.contains("unknown resource gpu"), "{err}");
    assert!(!rig.copy().join("ran").exists());
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_call_dropped_mid_run_frees_its_hold() {
    let rig = Rig::with_resources(&["build=1"]);
    rig.call("sync", json!({})).unwrap();
    let first = exec("touch $HOME/first-started; sleep 30", &["build"]);
    let call = rig.send_call("exec", first);
    wait_for(&rig.remote.join("first-started"));
    drop(call);
    let started = Instant::now();
    let out = rig.call("exec", exec("echo next", &["build"])).unwrap();
    assert_eq!(out["output"], "next\n");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_call_dropped_mid_wait_never_takes_the_hold() {
    let rig = Arc::new(Rig::with_resources(&["build=1"]));
    rig.call("sync", json!({})).unwrap();
    let holder = {
        let rig = Arc::clone(&rig);
        thread::spawn(move || rig.call("exec", exec("touch $HOME/held; sleep 3", &["build"])))
    };
    wait_for(&rig.remote.join("held"));
    let call = rig.send_call("exec", exec("touch $HOME/dropped-ran", &["build"]));
    thread::sleep(Duration::from_millis(500));
    drop(call);
    holder.join().unwrap().unwrap();
    let out = rig.call("exec", exec("echo after", &["build"])).unwrap();
    assert_eq!(out["output"], "after\n");
    thread::sleep(Duration::from_millis(500));
    assert!(
        !rig.remote.join("dropped-ran").exists(),
        "the dropped call never ran"
    );
}

#[test]
fn holds_taken_in_opposite_orders_never_deadlock() {
    let rig = Arc::new(Rig::with_resources(&["build=1", "sim=1"]));
    let started = Instant::now();
    let runs = concurrently(
        &rig,
        vec![
            ("12", "minion", exec("sleep 2", &["build"])),
            (
                "7",
                "minion",
                json!({"command": "true", "hold": ["build", "sim"], "hold_wait_secs": 20}),
            ),
            (
                "8",
                "minion",
                json!({"command": "true", "hold": ["sim", "build"], "hold_wait_secs": 20}),
            ),
        ],
    );
    for (result, _, _) in &runs {
        result.as_ref().unwrap();
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}
