//! Attaching through a fake controller socket: screen bytes out, typed keys in up to the
//! detach key, and the controller's end reason back.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "the fake controller panics its thread, which the test then sees"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use moor_client::Client;
use moor_core::controller::{write_bytes, ClientFrame, ServerFrame, PROTOCOL};
use moor_core::request::AttachReq;
use moor_core::response::Response;
use moor_core::view::AttachEnd;
use moor_herdr::card::Ended;
use moor_herdr::relay::{pump, Event};
use serde_json::json;

#[derive(Clone, Default)]
struct Screen(Arc<Mutex<Vec<u8>>>);

impl Write for Screen {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type Seen = (Vec<u8>, Vec<(u16, u16)>);

fn controller(listener: UnixListener) -> thread::JoinHandle<Seen> {
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        writer
            .write_all(
                ServerFrame::Hello { protocol: PROTOCOL }
                    .to_line()
                    .as_bytes(),
            )
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains(r#""cmd":"attach""#), "{line}");
        write_bytes(&mut writer, "1", b"agent screen").unwrap();
        let (mut keys, mut sizes) = (Vec::new(), Vec::new());
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            match serde_json::from_str::<ClientFrame>(&line).unwrap() {
                ClientFrame::Bytes { len, .. } => {
                    let mut buf = vec![0; len];
                    reader.read_exact(&mut buf).unwrap();
                    keys.extend(buf);
                }
                ClientFrame::Resize { cols, rows, .. } => sizes.push((cols, rows)),
                other => panic!("unexpected {other:?}"),
            }
        }
        let value = json!({"ended_by": "client", "bytes_out": 12, "bytes_in": keys.len()});
        let end = ServerFrame::Response {
            id: "1".into(),
            envelope: Response::ok("attach", Some(value)),
        };
        writer.write_all(end.to_line().as_bytes()).unwrap();
        (keys, sizes)
    })
}

#[test]
fn keys_reach_the_agent_until_the_detach_key_and_the_end_reason_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("controller.sock");
    let server = controller(UnixListener::bind(&socket).unwrap());
    let req: AttachReq =
        serde_json::from_value(json!({"box": "fix", "cols": 80, "rows": 24})).unwrap();
    let (screen, keys) = Client::new(&socket).attach(req).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(Event::Resized(100, 30)).unwrap();
    tx.send(Event::Keys(b"hi\x1dnot sent".to_vec())).unwrap();
    tx.send(Event::Keys(b"after detach".to_vec())).unwrap();
    let out = Screen::default();
    let ended = pump(screen, keys, (&tx, &rx), out.clone());
    assert_eq!(ended, Ended::Relay(AttachEnd::Client));
    assert_eq!(out.0.lock().unwrap().as_slice(), b"agent screen");
    let (typed, sizes) = server.join().unwrap();
    assert_eq!(typed, b"hi");
    assert_eq!(sizes, [(100, 30)]);
}
