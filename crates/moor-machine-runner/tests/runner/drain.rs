use std::time::{Duration, Instant};

use serde_json::json;

use crate::rig::Rig;

#[test]
fn sigterm_lets_an_exec_in_flight_finish_and_tells_the_agent_to_relink() {
    let mut rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let started = rig.copy().join("started");
    let (out, stopping) = std::thread::scope(|s| {
        let call = s.spawn(|| {
            rig.call(
                "exec",
                json!({"command": "touch started; sleep 2; echo done"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() {
            assert!(Instant::now() < deadline, "the exec never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        rig.sigterm();
        let stopping = Instant::now();
        (call.join().unwrap(), stopping)
    });
    let out = out.unwrap();
    assert_eq!(out["output"], "done\n", "{out}");
    assert_eq!(rig.exit_code(Duration::from_secs(10)), Some(0));
    assert!(
        stopping.elapsed() < Duration::from_secs(10),
        "the runner waited for the drain budget, not the stream"
    );
    let log = rig.agent_log.lock().unwrap().clone();
    assert!(
        log.iter().any(|l| l.contains("asked for a new link")),
        "{log:?}"
    );
}
