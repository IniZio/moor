use super::*;

#[test]
fn numbers_count_earlier_saves_only() {
    assert_eq!(next_number([]), 1);
    assert_eq!(
        next_number([
            "started",
            "transcript_saved",
            "transcript_failed",
            "transcript_saved"
        ]),
        3
    );
}

#[test]
fn path_names_task_session_and_number() {
    assert_eq!(relative_path(TaskId::new(4), 9, 2), "transcripts/4/9-2");
}

#[test]
fn digest_is_sha256_hex() {
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn triggers_are_named() {
    assert_eq!(Trigger::Relaunch.name(), "relaunch");
    assert_eq!(Trigger::Close.name(), "close");
}
