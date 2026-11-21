//! The check that is due, read from the lines of a gate.

use super::*;

#[test]
fn the_due_check_is_the_next_one_of_the_try_in_hand() {
    let g = gates(&[tries(2, doc("tests", &[], &["a", "b"], None))]);
    let due = |lines: &[GateLine]| due_check(&g, lines).map(|d| (d.try_n, d.check));
    assert_eq!(due(&[]), Some((1, 0)));
    assert_eq!(due(&[checked("tests", 1, 0, 0)]), Some((1, 1)));
    assert_eq!(
        due(&[checked("tests", 1, 0, 0), lost("tests", 1, 1)]),
        Some((1, 1))
    );
    let failed = [checked("tests", 1, 0, 0), checked("tests", 1, 1, 2)];
    assert_eq!(due(&failed), Some((2, 0)), "a failed try starts over");
    let spent = [
        failed[0].clone(),
        failed[1].clone(),
        checked("tests", 2, 0, 1),
    ];
    assert_eq!(due(&spent), None, "stuck");
    let reopen = [
        spent[0].clone(),
        spent[1].clone(),
        spent[2].clone(),
        reopened("tests"),
    ];
    assert_eq!(due(&reopen), Some((1, 0)), "reopened, so tried afresh");
    assert_eq!(due(&[reopened("tests")]), Some((1, 0)), "nothing to reopen");
    let both = [checked("tests", 1, 0, 0), checked("tests", 1, 1, 0)];
    assert_eq!(due(&both), None, "passed");
}
