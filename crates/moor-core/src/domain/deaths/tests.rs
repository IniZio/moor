use super::*;

#[test]
fn the_sentence_says_moor_did_not_resume_and_how_the_places_ended() {
    assert_eq!(
        sentence("the work place exited with code 137"),
        "its machine stopped, so moor did not resume it: the work place exited with code 137"
    );
}
