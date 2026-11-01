//! A loopback HTTP proxy that answers `CONNECT` and records each request line.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// What the proxy does with a `CONNECT`.
#[derive(Clone, Copy)]
pub(crate) enum Answer {
    /// Splices to the requested authority, or to the given address.
    Tunnel(Option<SocketAddr>),
    /// Answers `403 Forbidden` and closes.
    Refuse,
}

/// A running fake proxy.
pub(crate) struct FakeProxy {
    /// Where it listens.
    pub(crate) addr: SocketAddr,
    /// Request lines received, in order.
    pub(crate) seen: Arc<Mutex<Vec<String>>>,
}

impl FakeProxy {
    /// Starts a proxy that answers every `CONNECT` with `answer`.
    pub(crate) fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                thread::spawn(move || serve(client, answer, &log));
            }
        });
        Self { addr, seen }
    }

    /// `http://` URL of the proxy.
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Request lines received so far.
    pub(crate) fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(client: TcpStream, answer: Answer, log: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(client.try_clone().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    log.lock().unwrap().push(first.trim_end().to_owned());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
    }
    let mut out = client;
    let target = match answer {
        Answer::Refuse => {
            let _ = out.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        Answer::Tunnel(Some(to)) => TcpStream::connect(to),
        Answer::Tunnel(None) => {
            let authority = first.split_whitespace().nth(1).unwrap_or_default();
            TcpStream::connect(authority)
        }
    };
    let Ok(upstream) = target else {
        let _ = out.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
        return;
    };
    out.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .unwrap();
    let (mut up_r, mut up_w) = (upstream.try_clone().unwrap(), upstream);
    let mut down_w = out;
    thread::spawn(move || std::io::copy(&mut reader, &mut up_w));
    let _ = std::io::copy(&mut up_r, &mut down_w);
}
