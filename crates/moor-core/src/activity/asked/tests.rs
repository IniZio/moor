//! Tests for reading the question a reply asks.

use super::asked;

/// The end of a real report that put two questions to the person, then signed off.
const REPORT: &str = "The compose stack, emulator and Metro are still running on the Box.

What you need to do:
- [ ] Decide on the cryptography 48 -> 50 upgrade (alerts 217, 297, 300).

```
is this code? Recommend: no
```

Upgrade cryptography to 50.x in the server? Recommend: yes, then check with `make -C server ci`.

Open a PR to fix the 500 in `auth.py:44`? Recommend: yes.

Principles: Prove It Works, decided by building the bundle.";

#[test]
fn a_report_with_recommended_questions_asks_the_first_and_counts_the_rest() {
    assert_eq!(
        asked(REPORT).as_deref(),
        Some("Upgrade cryptography to 50.x in the server? (and 1 more)")
    );
}

#[test]
fn a_reply_ending_in_a_question_asks_it() {
    let said = "I fixed the test.\n\n**Should I open a PR?**";
    assert_eq!(asked(said).as_deref(), Some("Should I open a PR?"));
}

#[test]
fn a_numbered_question_with_a_lead_in_names_only_the_question() {
    let said = "1. #2 The bundle is clean. Drop d3-color? Recommend: yes";
    assert_eq!(asked(said).as_deref(), Some("Drop d3-color?"));
}

#[test]
fn a_report_that_asks_nothing_asks_nothing() {
    assert_eq!(
        asked("Android login verified. Waiting for iOS 26.4 cross-check."),
        None
    );
    assert_eq!(
        asked("Why did it fail? The cache was cold.\n\nFixed it."),
        None
    );
    assert_eq!(asked("```\nready?\n```"), None);
}
