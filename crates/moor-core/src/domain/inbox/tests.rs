use super::cut_note;
use crate::seam::clock::Ts;

/// 2026-11-17 20:22:17 UTC.
const SENT: i64 = 1_794_946_937_170;

const MIN: i64 = 60_000;

/// The note for `text` sent at [`SENT`], cut `mins` minutes later.
fn cut(mins: i64) -> String {
    cut_note(
        "rerun the suite",
        Ts::from_millis(SENT),
        Ts::from_millis(SENT + mins * MIN),
    )
}

#[test]
fn a_cut_note_names_when_its_text_was_first_sent_and_how_long_ago() {
    let note = cut(141);
    assert!(
        note.contains("(first sent 20:22 UTC, 2 h 21 min ago)"),
        "{note}"
    );
    assert!(note.ends_with("\n\n> rerun the suite"), "{note}");
}

#[test]
fn a_cut_note_under_an_hour_old_counts_minutes_only() {
    let note = cut(7);
    assert!(note.contains("(first sent 20:22 UTC, 7 min ago)"), "{note}");
}

#[test]
fn a_cut_note_a_day_old_names_the_date() {
    let note = cut(26 * 60);
    assert!(
        note.contains("(first sent 2026-11-17 20:22 UTC, 1 d 2 h ago)"),
        "{note}"
    );
}
