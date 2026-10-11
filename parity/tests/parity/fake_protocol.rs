use std::io::{Read, Write};
use std::process::Stdio;

use parity::fake::Rule;
use parity::parity;

fn pipe_through(w: &parity::world::World, input: Vec<u8>) -> Vec<u8> {
    let mut child = w
        .command(w.tool_path("herdr"))
        .arg("up")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn fake herdr");
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&input).unwrap();
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    writer.join().unwrap();
    assert!(child.wait().unwrap().success());
    out
}

parity!(fake_exec_cat_round_trips_stdin, covers = [], tags = [], {
    let w = crate::world();
    w.add_rules("herdr", [Rule::on(&["up"]).exec("cat").capture_stdin()]);
    let input: Vec<u8> = (0..200 * 1024).map(|i| b'a' + (i % 26) as u8).collect();
    let out = pipe_through(&w, input.clone());
    assert_eq!(out, input);
    let calls = w.calls("herdr");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].stdin.as_bytes(), &input[..]);
});

parity!(
    fake_exec_without_capture_still_gets_stdin,
    covers = [],
    tags = [],
    {
        let w = crate::world();
        w.add_rules("herdr", [Rule::on(&["up"]).exec("cat")]);
        let input = b"hello stdin\n".to_vec();
        let out = pipe_through(&w, input.clone());
        assert_eq!(out, input);
        assert_eq!(w.calls("herdr").len(), 1);
    }
);

parity!(fake_missing_fakes_dir_fails, covers = [], tags = [], {
    let w = crate::world();
    let status = w
        .command(w.tool_path("herdr"))
        .env("PARITY_FAKES", "/nonexistent-dir")
        .arg("up")
        .stderr(Stdio::null())
        .status()
        .expect("run fake");
    assert_eq!(status.code(), Some(97));
});

parity!(fake_records_pgid_and_starttime, covers = [], tags = [], {
    let w = crate::world();
    let status = w
        .command(w.tool_path("herdr"))
        .arg("up")
        .status()
        .expect("run fake");
    assert!(status.success());
    let calls = w.calls("herdr");
    assert_eq!(calls.len(), 1);
    assert_ne!(calls[0].pgid, 0);
    assert_ne!(calls[0].starttime, 0);
});
