//! A row that asked for a cold start asks the controller for one.

use super::*;

#[test]
fn a_cold_row_starts_cold() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let mut row = recorded_start(&db);
    row.cold = true;
    let seen = attempt_with(&db, &row, Attempt::First, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    let Some(Request::Start(req)) = seen.calls.first() else {
        panic!("no start: {:?}", cmds(&seen));
    };
    assert!(req.cold);
}
