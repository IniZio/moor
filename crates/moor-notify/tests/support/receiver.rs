//! A webhook receiver on the loopback: it records each POST and answers with the next
//! scripted status, 200 once the script runs out.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One POST the receiver got.
#[derive(Debug, Clone)]
pub(crate) struct Posted {
    /// Header names lowercased, to their values.
    pub(crate) headers: HashMap<String, String>,
    /// The raw body.
    pub(crate) body: Vec<u8>,
    /// The status it was answered with.
    pub(crate) status: u16,
}

/// State shared with the connection threads.
#[derive(Default)]
struct Inner {
    /// Statuses to answer with, in order.
    script: VecDeque<u16>,
    /// What arrived.
    posted: Vec<Posted>,
}

/// A running receiver.
pub(crate) struct Receiver {
    /// Its URL.
    pub(crate) url: String,
    /// State shared with the connection threads.
    inner: Arc<Mutex<Inner>>,
}

impl Receiver {
    /// Listens on a free loopback port and answers with `script` first.
    pub(crate) fn start(script: &[u16]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}/hook",
            listener.local_addr().unwrap().port()
        );
        let inner = Arc::new(Mutex::new(Inner {
            script: script.iter().copied().collect(),
            posted: Vec::new(),
        }));
        let shared = Arc::clone(&inner);
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let inner = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &inner));
            }
        });
        Self { url, inner }
    }

    /// Every POST so far.
    pub(crate) fn posted(&self) -> Vec<Posted> {
        self.inner.lock().unwrap().posted.clone()
    }

    /// Waits until `n` POSTs arrived; fails the test after 30 s.
    pub(crate) fn wait_for(&self, n: usize) -> Vec<Posted> {
        for _ in 0..1500 {
            let posted = self.posted();
            if posted.len() >= n {
                return posted;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("receiver got {:?}, wanted {n} posts", self.posted().len());
    }
}

/// Serves each request on one keep-alive connection.
fn serve(stream: TcpStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    loop {
        let mut start = String::new();
        if reader.read_line(&mut start).unwrap_or(0) == 0 {
            return;
        }
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
        let length: usize = headers
            .get("content-length")
            .map_or(0, |n| n.parse().unwrap());
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let status = {
            let mut inner = inner.lock().unwrap();
            let status = inner.script.pop_front().unwrap_or(200);
            inner.posted.push(Posted {
                headers,
                body,
                status,
            });
            status
        };
        let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n");
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}
