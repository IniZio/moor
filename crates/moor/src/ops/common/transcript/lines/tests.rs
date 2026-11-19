use std::io::Write as _;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use moor_core::progress::transcript::What;
use serde_json::json;

use super::{is_session, read};

fn record(id: &str, command: &str) -> String {
    json!({"type": "assistant", "uuid": format!("u-{id}"), "timestamp": "2026-11-19T10:00:00.000Z",
           "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash",
                                    "input": {"command": command}}]}})
    .to_string()
}

fn tgz(dir: &Path, files: &[(&str, String)]) -> std::path::PathBuf {
    let path = dir.join("a.tgz");
    let gz = GzEncoder::new(std::fs::File::create(&path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(gz);
    for (name, text) in files {
        let mut head = tar::Header::new_gnu();
        head.set_size(text.len() as u64);
        head.set_mode(0o600);
        head.set_cksum();
        tar.append_data(&mut head, name, text.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap().flush().unwrap();
    path
}

#[test]
fn only_the_session_files_at_the_top_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let archive = tgz(
        dir.path(),
        &[
            (
                "./s1.jsonl",
                format!("{}\nnot json\n", record("t1", "npm test")),
            ),
            ("./agent-x.jsonl", record("t2", "from a subagent")),
            ("./s1/subagents/agent-y.jsonl", record("t3", "nested")),
            ("./notes.txt", record("t4", "not a session")),
        ],
    );
    let lines = read(&archive).unwrap();
    let titles: Vec<String> = lines
        .iter()
        .filter_map(|l| match &l.what {
            What::Tool(t) => Some(t.title.clone()),
            What::Said { .. } | What::Typed { .. } => None,
        })
        .collect();
    assert_eq!(titles, ["Ran npm test"]);
}

#[test]
fn an_archive_that_is_not_gzip_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.tgz");
    std::fs::write(&path, "abc").unwrap();
    assert!(read(&path).is_err());
}

#[test]
fn a_session_file_is_one_at_the_top_named_jsonl() {
    assert!(is_session(Path::new("./abc.jsonl")));
    assert!(is_session(Path::new("abc.jsonl")));
    assert!(!is_session(Path::new("./agent-1.jsonl")));
    assert!(!is_session(Path::new("./abc/def.jsonl")));
    assert!(!is_session(Path::new("./abc.json")));
}
