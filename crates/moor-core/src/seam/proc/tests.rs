use super::*;

#[test]
fn tool_names() {
    assert_eq!(Tool::Agent.bin_var(), "MOOR_CLAUDE_BIN");
    assert_eq!(Tool::Agent.program(), "claude");
    assert_eq!(Tool::MoorSelf.program(), "moor");
}

#[test]
fn spec_builder() {
    let s = ProcSpec::new(Tool::Git, Deadline(Duration::from_secs(5)))
        .arg("status")
        .args(["-s", "-b"])
        .cwd("/tmp")
        .env("K", "V");
    assert_eq!(s.arg_strings(), ["status", "-s", "-b"]);
    assert_eq!(s.env.extra, [("K".to_owned(), "V".to_owned())]);
    assert!(matches!(s.stdin, Stdin::Null));
}

#[test]
fn stdin_debug_hides_secret() {
    let d = format!("{:?}", Stdin::Secret(Secret::new(b"hunter2".to_vec())));
    assert!(!d.contains("hunter2"));
}

#[test]
fn error_display_has_no_stderr() {
    let e = ProcError::NonZero {
        code: 3,
        stdout: String::new(),
        stderr_tail: "token abc".into(),
    };
    assert_eq!(e.to_string(), "program exited with code 3");
}

#[test]
fn first_line_error_display() {
    let t = FirstLineError::Timeout {
        tool: Tool::Ssh,
        after: Duration::from_secs(5),
    };
    assert_eq!(t.to_string(), "ssh gave no first line within 5s");
    let e = FirstLineError::Exited {
        code: Some(2),
        signal: None,
        stderr_tail: "secret".into(),
    };
    assert_eq!(
        e.to_string(),
        "program exited with code 2 before its first line"
    );
    let s = FirstLineError::Exited {
        code: None,
        signal: Some(9),
        stderr_tail: String::new(),
    };
    assert_eq!(
        s.to_string(),
        "program killed by signal 9 before its first line"
    );
}

#[test]
fn interrupted_display_is_plain() {
    assert_eq!(ProcError::Interrupted.to_string(), "interrupted");
}
