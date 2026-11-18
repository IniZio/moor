use super::*;

const MIN: i64 = 60_000;

#[test]
fn the_third_death_inside_the_window_is_capped() {
    assert_eq!(
        record(&[], 0),
        Tally {
            kept: vec![0],
            capped: false
        }
    );
    assert!(!record(&[0], 10 * MIN).capped);
    let third = record(&[0, 10 * MIN], 30 * MIN);
    assert_eq!(third.kept, vec![0, 10 * MIN, 30 * MIN]);
    assert!(third.capped);
}

#[test]
fn a_death_outside_the_window_is_forgotten() {
    let out = record(&[0, 10 * MIN], 30 * MIN + 1);
    assert_eq!(out.kept, vec![10 * MIN, 30 * MIN + 1]);
    assert!(!out.capped);
}

#[test]
fn only_the_newest_deaths_are_kept_and_a_stale_clock_counts_nothing() {
    let out = record(&[MIN, 2 * MIN, 3 * MIN], 4 * MIN);
    assert_eq!(out.kept, vec![2 * MIN, 3 * MIN, 4 * MIN]);
    assert!(!record(&[10 * MIN], 5 * MIN).capped);
}

#[test]
fn the_sentence_names_the_count_the_window_and_the_last_death() {
    assert_eq!(
        sentence(3, "the work place exited with code 137"),
        "its machine stopped 3 times in 30 minutes, so moor stopped resuming it: the work place exited with code 137"
    );
}
