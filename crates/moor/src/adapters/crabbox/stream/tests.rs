//! Tests for streaming Box files as one tar archive over a single ssh process.

use std::io::{self, Cursor, Read};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::{Deadline, Tool};

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

const SSH_LINE: &str = "ssh -p 2222 user@1.2.3.4\n";
const BINARY: [u8; 5] = [0xff, 0xfe, 0x00, 0x80, 0x0a];
const BIG: u64 = 50 * 1024 * 1024;

/// Bytes pulled from the counted source so far.
static PULLED: AtomicUsize = AtomicUsize::new(0);

struct Counted<R>(R);

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.0.read(buf)?;
        PULLED.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}

type Got = Vec<(String, Option<Vec<u8>>)>;

fn fixture() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    (p, c)
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Run `stream_files` and collect what the sink was handed, fully read.
fn stream(c: &Crabbox, wanted: &[&str]) -> (Result<(), DriverError>, Got) {
    let mut got: Got = Vec::new();
    let mut sink = |name: &str, reader: Option<&mut dyn Read>| {
        let body = reader.map(|r| {
            let mut buf = Vec::new();
            r.read_to_end(&mut buf).unwrap();
            buf
        });
        got.push((name.to_owned(), body));
    };
    let out = c.stream_files(
        &BoxHandle::new("p/box"),
        &BoxPath::under("/work", "out").unwrap(),
        &names(wanted),
        Path::new("/host/cwd"),
        Deadline(Duration::from_secs(60)),
        &mut sink,
    );
    (out, got)
}

fn file_header(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_size(size);
    h.set_mode(0o644);
    h
}

/// A finished tar archive of regular files.
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, data) in entries {
        let mut h = file_header(data.len() as u64);
        h.set_cksum();
        b.append_data(&mut h, path, *data).unwrap();
    }
    b.into_inner().unwrap()
}

fn reader_of(bytes: Vec<u8>) -> Box<dyn Read + Send> {
    Box::new(Cursor::new(bytes))
}

fn three_files() -> Box<dyn Read + Send> {
    reader_of(archive(&[
        ("a.txt", b"alpha"),
        ("bin.dat", &BINARY),
        ("c.txt", b"gamma"),
    ]))
}

fn truncated() -> Box<dyn Read + Send> {
    let mut bytes = archive(&[("a.txt", b"alpha"), ("b.txt", &[b'b'; 2000])]);
    // a: header + one data block, then b's header and part of its data.
    bytes.truncate(1024 + 512 + 1000);
    reader_of(bytes)
}

fn with_symlink() -> Box<dyn Read + Send> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_cksum();
    b.append_link(&mut h, "link", "target").unwrap();
    let mut h = file_header(6);
    h.set_cksum();
    b.append_data(&mut h, "extra.txt", &b"hidden"[..]).unwrap();
    let mut h = file_header(2);
    h.set_cksum();
    b.append_data(&mut h, "keep.txt", &b"ok"[..]).unwrap();
    reader_of(b.into_inner().unwrap())
}

/// A 50 MiB entry generated lazily; nothing holds the whole body.
fn big_entry() -> Box<dyn Read + Send> {
    let mut h = file_header(BIG);
    h.set_path("big.bin").unwrap();
    h.set_cksum();
    let pad = vec![0u8; (512 - (BIG % 512) as usize) % 512 + 1024];
    Box::new(Counted(
        Cursor::new(h.as_bytes().to_vec())
            .chain(io::repeat(b'x').take(BIG))
            .chain(Cursor::new(pad)),
    ))
}

#[test]
fn entries_arrive_byte_exact_in_order_over_one_ssh() {
    let (p, c) = fixture();
    p.on(Tool::Ssh, &[], Reply::Reader(three_files));
    let (out, got) = stream(&c, &["a.txt", "bin.dat", "c.txt"]);
    out.unwrap();
    assert_eq!(
        got,
        [
            ("a.txt".to_owned(), Some(b"alpha".to_vec())),
            ("bin.dat".to_owned(), Some(BINARY.to_vec())),
            ("c.txt".to_owned(), Some(b"gamma".to_vec())),
        ]
    );
    let calls = p.calls();
    assert!(!calls.iter().any(|c| c.tool == Tool::Crabbox
        && matches!(c.args.first().map(String::as_str), Some("cp" | "run"))));
    let ssh: Vec<_> = calls.iter().filter(|c| c.tool == Tool::Ssh).collect();
    assert_eq!(ssh.len(), 1);
    let remote = ssh.first().unwrap().args.last().unwrap();
    assert!(remote.contains("tar -cf -"), "{remote}");
    for n in ["'a.txt'", "'bin.dat'", "'c.txt'"] {
        assert!(remote.contains(n), "{remote}");
    }
}

#[test]
fn ssh_target_is_looked_up_once_per_box() {
    let (p, c) = fixture();
    p.on(Tool::Ssh, &[], Reply::Reader(three_files));
    stream(&c, &["a.txt"]).0.unwrap();
    stream(&c, &["a.txt"]).0.unwrap();
    let lookups = p
        .calls()
        .iter()
        .filter(|c| c.tool == Tool::Crabbox && c.args.first().map(String::as_str) == Some("ssh"))
        .count();
    assert_eq!(lookups, 1);
}

#[test]
fn truncated_stream_fails_but_keeps_earlier_files() {
    let (p, c) = fixture();
    p.on(Tool::Ssh, &[], Reply::Reader(truncated));
    let (out, got) = stream(&c, &["a.txt", "b.txt"]);
    assert!(matches!(out, Err(DriverError::Failed { .. })), "{out:?}");
    assert_eq!(
        got.first(),
        Some(&("a.txt".to_owned(), Some(b"alpha".to_vec())))
    );
}

#[test]
fn box_without_tar_is_unsupported() {
    let (p, c) = fixture();
    p.on(
        Tool::Ssh,
        &[],
        Reply::Fail {
            code: super::NO_TAR,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    let (out, got) = stream(&c, &["a.txt"]);
    assert!(matches!(out, Err(DriverError::Unsupported(_))), "{out:?}");
    assert!(got.is_empty());
}

#[test]
fn non_regular_entry_has_no_reader_and_unrequested_entries_are_skipped() {
    let (p, c) = fixture();
    p.on(Tool::Ssh, &[], Reply::Reader(with_symlink));
    let (out, got) = stream(&c, &["link", "keep.txt"]);
    out.unwrap();
    assert_eq!(
        got,
        [
            ("link".to_owned(), None),
            ("keep.txt".to_owned(), Some(b"ok".to_vec())),
        ]
    );
}

#[test]
fn no_names_means_no_process_at_all() {
    let (p, c) = fixture();
    let (out, got) = stream(&c, &[]);
    out.unwrap();
    assert!(got.is_empty());
    assert!(p.calls().is_empty());
}

#[test]
fn large_entry_streams_without_being_buffered() {
    let (p, c) = fixture();
    PULLED.store(0, Ordering::SeqCst);
    p.on(Tool::Ssh, &[], Reply::Reader(big_entry));
    let mut seen = None;
    let mut sink = |name: &str, reader: Option<&mut dyn Read>| {
        assert_eq!(name, "big.bin");
        let reader = reader.unwrap();
        let mut first = [0u8; 4096];
        let head = reader.read(&mut first).unwrap();
        let pulled = PULLED.load(Ordering::SeqCst);
        assert!(
            pulled < 1024 * 1024,
            "buffered {pulled} bytes before the sink read"
        );
        seen = Some(head as u64 + io::copy(reader, &mut io::sink()).unwrap());
    };
    c.stream_files(
        &BoxHandle::new("p/box"),
        &BoxPath::under("/work", "out").unwrap(),
        &names(&["big.bin"]),
        Path::new("/host/cwd"),
        Deadline(Duration::from_secs(60)),
        &mut sink,
    )
    .unwrap();
    assert_eq!(seen, Some(BIG));
}
