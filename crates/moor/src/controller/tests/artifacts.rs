#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]
//! A web-style client over the controller socket: list artifacts, stream their raw bytes.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use moor_core::controller::{BYTES_MAX, PROTOCOL};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::name::BoxName;
use moor_core::{ArtifactReadReq, ArtifactsReq, Request};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::controller::{start, Sink};
use crate::env::Env;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use moor_core::seam::clock::Ts;

const BOX_NAME: &str = "web-box";
const OLD_LEN: usize = 5;
const BIG_LEN: usize = 1024 * 1024 + 100;

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
    let name = BoxName::parse_allow_bare(BOX_NAME).expect("name");
    let id = store
        .apply(Decision::insert(NewBox {
            name,
            repo: "r".into(),
        }))
        .expect("box")
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
    /// Send one request; return the response envelope and the payload of each bytes frame
    /// that came before it.
    fn call(&mut self, request: &Request) -> (Value, Vec<Vec<u8>>) {
        self.next += 1;
        let id = format!("r{}", self.next);
        let frame = json!({"frame": "request", "id": id, "request": request});
        writeln!(self.stream.get_mut(), "{frame}").expect("send");
        let mut frames = Vec::new();
        loop {
            let mut line = String::new();
            self.stream.read_line(&mut line).expect("read");
            let v: Value = serde_json::from_str(&line).expect("json frame");
            match v["frame"].as_str() {
                Some("bytes") => {
                    assert_eq!(v["id"], id);
                    let len = usize::try_from(v["len"].as_u64().expect("len")).expect("fits");
                    let mut payload = vec![0; len];
                    self.stream.read_exact(&mut payload).expect("payload");
                    frames.push(payload);
                }
                Some("response") => {
                    assert_eq!(v["id"], id);
                    return (v["envelope"].clone(), frames);
                }
                _ => {}
            }
        }
    }

    fn list(&mut self, all: bool) -> Vec<Value> {
        let (env, _) = self.call(&Request::Artifacts(ArtifactsReq {
            box_name: box_name(),
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

    fn read(&mut self, name: &str, version: Option<u32>) -> (Value, Vec<Vec<u8>>) {
        self.call(&Request::ArtifactRead(ArtifactReadReq {
            box_name: box_name(),
            name: name.to_owned(),
            version,
            offset: 0,
            len: u64::MAX,
        }))
    }
}

fn box_name() -> BoxName {
    BoxName::parse_allow_bare(BOX_NAME).expect("name")
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
fn a_read_streams_raw_bytes_frames_matching_the_recorded_digest() {
    let mut f = fixture();
    let listed = entry(&f.list(false), "demo.log", 2)["sha256"].clone();
    let (env, frames) = f.read("demo.log", None);
    assert_eq!(env["class"], "ok", "{env}");
    assert!(frames.iter().all(|p| p.len() <= BYTES_MAX));
    assert_eq!(frames.len(), BIG_LEN.div_ceil(64 * 1024));
    assert_eq!(frames.concat(), f.big);
    let whole = json!({"offset": 0, "sent": BIG_LEN, "size": BIG_LEN, "sha256": hex(&f.big),
                       "eof": true});
    assert_eq!(env["value"], whole);
    assert_eq!(listed, whole["sha256"]);
    assert_eq!(f.list(false).len(), 2, "the connection is still in step");
}

#[test]
fn explicit_version_reads_the_old_bytes() {
    let mut f = fixture();
    let (env, frames) = f.read("demo.log", Some(1));
    assert_eq!(frames.concat(), b"old-v");
    assert_eq!(env["value"]["sha256"], hex(b"old-v").as_str());
}

#[test]
fn missing_box_name_or_version_is_not_found() {
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
    let (env, frames) = f.read("nope.log", None);
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
    assert!(frames.is_empty());
    let (env, _) = f.read("demo.log", Some(9));
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
}
