//! A fake GitHub API on a loopback port: `GET /user` answers a status the test sets, and the
//! fake records what it was asked.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// One request the fake received.
#[derive(Clone, Debug)]
pub(crate) struct Asked {
    /// Request line, such as `GET /user HTTP/1.1`.
    pub(crate) line: String,
    /// The `Authorization` header value, empty when absent.
    pub(crate) auth: String,
    /// The `User-Agent` header value, empty when absent.
    pub(crate) user_agent: String,
}

/// What the fake answers with and has seen.
struct State {
    /// HTTP status of the next answers.
    status: u16,
    /// Every request, in arrival order.
    asked: Vec<Asked>,
}

/// The fake GitHub API.
pub(crate) struct FakeGitHub {
    /// Shared with the serving threads.
    state: Arc<Mutex<State>>,
    /// Port of the listener.
    port: u16,
}

impl FakeGitHub {
    /// Starts the listener on a loopback port; it answers 200 until told otherwise.
    pub(crate) fn start() -> Self {
        let state = Arc::new(Mutex::new(State {
            status: 200,
            asked: Vec::new(),
        }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { state, port }
    }

    /// The `github_api_base` to put in the front's config.
    pub(crate) fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Answers every later request with HTTP `status`.
    pub(crate) fn answer(&self, status: u16) {
        self.lock().status = status;
    }

    /// Every request seen so far.
    pub(crate) fn asked(&self) -> Vec<Asked> {
        self.lock().asked.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

/// Reads one request head, records it and answers.
fn serve(mut stream: TcpStream, state: &Mutex<State>) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(n) if n > 0 => head.extend_from_slice(&chunk[..n]),
            _ => return,
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or_default().to_owned();
    let header = |name: &str| {
        lines
            .clone()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
            .unwrap_or_default()
    };
    let asked = Asked {
        line,
        auth: header("authorization"),
        user_agent: header("user-agent"),
    };
    let status = {
        let mut guard = state.lock().unwrap();
        guard.asked.push(asked);
        guard.status
    };
    let body = r#"{"login":"octocat"}"#;
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}
