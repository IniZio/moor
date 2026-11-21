//! What an answer is read at, and how a failed check is named.

use super::*;

#[test]
fn a_replayed_answer_for_an_earlier_try_changes_nothing() {
    let g = gates(&[tries(3, doc("ok", &[], &[], Some(Approval::Owner)))]);
    let once = [answered("ok", 1, Answer::No)];
    let replay = [answered("ok", 1, Answer::No), answered("ok", 1, Answer::No)];
    assert_eq!(status(&g, &replay), open("ok", 2, GateNext::Ask));
    assert_eq!(status(&g, &replay), status(&g, &once));
    let yes_for_the_old_try = [
        answered("ok", 1, Answer::No),
        answered("ok", 1, Answer::Yes),
    ];
    assert_eq!(
        status(&g, &yes_for_the_old_try),
        open("ok", 2, GateNext::Ask)
    );
}

#[test]
fn a_failed_check_is_named_by_its_command() {
    let g = gates(&[tries(
        1,
        doc("tests", &[], &["cargo test --workspace"], None),
    )]);
    let long = "x".repeat(80);
    let h = gates(&[tries(1, doc("tests", &[], &[long.as_str()], None))]);
    let got = status(&g, &[checked("tests", 1, 0, 101)]);
    assert_eq!(
        got,
        stuck(
            "tests",
            "out of tries: check 1 (`cargo test --workspace`) exited 101"
        )
    );
    let got = status(&h, &[lost("tests", 1, 0), lost("tests", 1, 0)]);
    let want = format!(
        "out of tries: check 1 (`{}…`) lost its output twice",
        "x".repeat(60)
    );
    assert_eq!(got, stuck("tests", &want));
}
