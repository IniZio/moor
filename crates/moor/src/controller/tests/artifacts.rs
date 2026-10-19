#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]
//! A web-style client over the controller socket: list artifacts, read them in chunks.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use moor_core::controller::PROTOCOL;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::name::BoxName;
use moor_core::{ArtifactReadReq, ArtifactsReq, Request, ARTIFACT_CHUNK};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::controller::{start, Sink};
use crate::env::Env;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use moor_core::seam::clock::Ts;

const TASK: &str = "web-box";
const OLD_LEN: usize = 5;
const CHUNK: usize = 512 * 1024;
const BIG_LEN: usize = CHUNK * 2 + 100 * 1024;

fn patterned(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from(i % 251).expect("fits"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Fixture {
    _dir: tempfile::TempDir,
    stream: BufReader<UnixStream>,
    big: Vec<u8>,
    next: u32,
}

#[derive(Clone, Default)]
struct Quiet;

impl Write for Quiet {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn seed(dir: &Path) -> Vec<u8> {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.join("moor.db"), clock).expect("store");
    let name = BoxName::parse_allow_bare(TASK).expect("name");
    let id = store
        .apply(Decision::insert(NewBox {
            name,
            repo: "r".into(),
        }))
        .expect("task")
        .box_id;
    let root = dir.join("artifacts");
    let big = patterned(BIG_LEN);
    store
        .add_artifact(id, "demo.log", b"old-v", &root)
        .expect("v1");
    store.add_artifact(id, "demo.log", &big, &root).expect("v2");
    store
        .add_artifact(id, "shot.png", b"png", &root)
        .expect("shot");
    big
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tmp");
    let big = seed(dir.path());
    let cfg = dir.path().join("controller.yaml");
    std::fs::write(&cfg, "").expect("cfg");
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().expect("utf8")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let running = start(&env, &cfg).expect("start");
    let socket = running.socket.clone();
    let sink: Sink = Arc::new(Mutex::new(Quiet));
    std::thread::spawn(move || running.accept_loop(&sink));
    let mut write = UnixStream::connect(&socket).expect("connect");
    let hello = json!({"frame": "hello", "protocol": PROTOCOL});
    writeln!(write, "{hello}").expect("hello");
    let mut stream = BufReader::new(write);
    let mut line = String::new();
    stream.read_line(&mut line).expect("hello reply");
    assert!(line.contains("\"hello\""), "{line}");
    Fixture {
        _dir: dir,
        stream,
        big,
        next: 0,
    }
}

impl Fixture {
    /// Send one request; return the response envelope and the frame's byte length.
    fn call(&mut self, request: &Request) -> (Value, usize) {
        self.next += 1;
        let id = format!("r{}", self.next);
        let frame = json!({"frame": "request", "id": id, "request": request});
        writeln!(self.stream.get_mut(), "{frame}").expect("send");
        loop {
            let mut line = String::new();
            self.stream.read_line(&mut line).expect("read");
            let v: Value = serde_json::from_str(&line).expect("json frame");
            if v["frame"] == "response" {
                assert_eq!(v["id"], id);
                return (v["envelope"].clone(), line.len());
            }
        }
    }

    fn list(&mut self, all: bool) -> Vec<Value> {
        let (env, _) = self.call(&Request::Artifacts(ArtifactsReq {
            box_name: task(),
            all,
        }));
        assert_eq!(env["class"], "ok", "{env}");
        let value = &env["value"];
        value
            .get("artifacts")
            .unwrap_or(value)
            .as_array()
            .expect("artifact list")
            .clone()
    }

    fn read(&mut self, name: &str, version: Option<u32>, offset: u64) -> (Value, usize) {
        self.call(&Request::ArtifactRead(ArtifactReadReq {
            box_name: task(),
            name: name.to_owned(),
            version,
            offset,
            len: 1024 * 1024,
        }))
    }

    /// Read a whole artifact chunk by chunk, checking each frame stays small.
    fn read_all(&mut self, name: &str, version: Option<u32>) -> (Vec<u8>, Value, usize) {
        let (mut bytes, mut chunks) = (Vec::new(), 0);
        loop {
            let (env, frame_len) = self.read(name, version, bytes.len() as u64);
            assert_eq!(env["class"], "ok", "{env}");
            assert!(frame_len < 1024 * 1024, "frame of {frame_len} bytes");
            let chunk = &env["value"];
            let part = base64::engine::general_purpose::STANDARD
                .decode(chunk["data_b64"].as_str().expect("data_b64"))
                .expect("base64");
            assert!(part.len() <= CHUNK);
            bytes.extend_from_slice(&part);
            chunks += 1;
            if chunk["eof"] == true {
                return (bytes, chunk.clone(), chunks);
            }
            assert!(!part.is_empty(), "no progress before eof");
        }
    }
}

fn task() -> BoxName {
    BoxName::parse_allow_bare(TASK).expect("name")
}

fn entry<'a>(list: &'a [Value], name: &str, version: u64) -> &'a Value {
    list.iter()
        .find(|e| e["name"] == name && e["version"] == version)
        .expect("entry listed")
}

#[test]
fn latest_lists_one_entry_per_name_and_all_lists_every_version() {
    let mut f = fixture();
    let latest = f.list(false);
    assert_eq!(latest.len(), 2, "{latest:?}");
    assert_eq!(entry(&latest, "demo.log", 2)["size"], BIG_LEN);
    entry(&latest, "shot.png", 1);
    let all = f.list(true);
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(entry(&all, "demo.log", 1)["size"], OLD_LEN);
}

#[test]
fn chunked_read_reassembles_bytes_matching_the_recorded_digest() {
    let mut f = fixture();
    let listed = entry(&f.list(false), "demo.log", 2)["sha256"].clone();
    let (bytes, last, chunks) = f.read_all("demo.log", None);
    assert_eq!(u64::try_from(CHUNK).expect("fits"), ARTIFACT_CHUNK);
    assert_eq!(chunks, 3);
    assert_eq!(bytes, f.big);
    assert_eq!(hex(&bytes), last["sha256"].as_str().expect("sha"));
    assert_eq!(listed, last["sha256"]);
    assert_eq!(last["size"], BIG_LEN);
}

#[test]
fn explicit_version_reads_the_old_bytes() {
    let mut f = fixture();
    let (bytes, last, chunks) = f.read_all("demo.log", Some(1));
    assert_eq!((bytes.as_slice(), chunks), (b"old-v".as_slice(), 1));
    assert_eq!(hex(b"old-v"), last["sha256"].as_str().expect("sha"));
}

#[test]
fn missing_task_name_or_version_is_not_found() {
    let mut f = fixture();
    let ghost = BoxName::parse_allow_bare("ghost").expect("name");
    let (env, _) = f.call(&Request::Artifacts(ArtifactsReq {
        box_name: ghost.clone(),
        all: false,
    }));
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
    let (env, _) = f.call(&Request::ArtifactRead(ArtifactReadReq {
        box_name: ghost,
        name: "demo.log".to_owned(),
        version: None,
        offset: 0,
        len: 10,
    }));
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
    let (env, _) = f.read("nope.log", None, 0);
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
    let (env, _) = f.read("demo.log", Some(9), 0);
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
}
