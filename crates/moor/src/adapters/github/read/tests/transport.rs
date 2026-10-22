//! The read's HTTP transport: tokens, redirects, proxies, timeouts and the reply cap.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, RepoAt};
use moor_core::Secret;

use super::{assert_no_token_fragment, github, token, FakeApi, TOKEN};
use crate::adapters::github::GitHub;

/// A read timeout short enough that a test waiting it out stays fast.
const SHORT: Duration = Duration::from_secs(2);

fn read_with(api: &str, timeout: Duration, token: &Secret) -> Result<RepoAt, ForgeError> {
    let github = GitHub {
        api: api.to_owned(),
        timeout,
    };
    github.read(&ForgeRepo::new("o", "n"), None, "moor/t", token)
}

/// A loopback server that reads one request, then hands its stream to `reply`.
fn raw_server(reply: impl FnOnce(TcpStream) + Send + 'static) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    let served = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            if line == "\r\n" {
                break;
            }
        }
        reader.read_exact(&mut vec![0; length]).unwrap();
        reply(reader.into_inner());
    });
    (url, served)
}

#[test]
fn unreachable_api_error_does_not_carry_the_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    drop(listener);
    let err = GitHub::at(url)
        .read(&ForgeRepo::new("o", "n"), None, "moor/t", &token())
        .unwrap_err();
    assert!(matches!(err, ForgeError::Unavailable(_)), "{err:?}");
    assert_no_token_fragment(&format!("{err} {err:?}"));
}

#[test]
fn token_that_is_not_printable_ascii_is_rejected_unsent() {
    let bad: [&[u8]; 9] = [
        b"",
        b"ghp_a b",
        "ghp_\u{e9}".as_bytes(),
        b"ghp_a\x7fb",
        b"ghp_a\x00b",
        b"ghp_a\tb",
        b"ghp_a\r\nX-Injected: 1",
        b"ghp_a\rb",
        b"ghp_a\nb",
    ];
    for raw in bad {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let r = read_with(&url, SHORT, &Secret::new(raw.to_vec()));
        assert_eq!(r, Err(ForgeError::Rejected), "{raw:?}");
        assert!(listener.accept().is_err(), "{raw:?} was sent");
    }
}

#[test]
fn redirect_is_not_followed_and_its_location_is_not_shown() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let port = target.local_addr().unwrap().port();
    let (url, served) = raw_server(move |mut s| {
        let location = format!("http://127.0.0.1:{port}/steal?t={TOKEN}");
        let _ = write!(
            s,
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    });
    let r = read_with(&url, SHORT, &token());
    served.join().unwrap();
    assert!(target.accept().is_err(), "the redirect was followed");
    assert_eq!(
        r,
        Err(ForgeError::Unavailable(
            "GitHub answered HTTP 302".to_owned()
        ))
    );
}

/// Body of the child process run by [`env_proxies_are_ignored`].
#[test]
#[ignore = "runs only as the child of another test"]
fn read_under_env_proxies() {
    let api = FakeApi::start(|v| github(v, "WRITE"));
    read_with(&api.url, SHORT, &token()).unwrap();
}

#[test]
fn env_proxies_are_ignored() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args([
        "--ignored",
        "--exact",
        "adapters::github::read::tests::transport::read_under_env_proxies",
    ]);
    for var in ["ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY"] {
        child
            .env(var, &proxy_url)
            .env(var.to_ascii_lowercase(), &proxy_url);
    }
    let out = child
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{stdout}"
    );
    assert!(proxy.accept().is_err(), "the read went through the proxy");
}

#[test]
#[expect(clippy::disallowed_methods, reason = "times a real stall")]
fn stalled_reply_times_out() {
    let (url, served) = raw_server(|mut s| {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let _ = s.read(&mut [0; 1]);
    });
    let started = Instant::now();
    let r = read_with(&url, Duration::from_millis(300), &token());
    let waited = started.elapsed();
    served.join().unwrap();
    assert!(matches!(r, Err(ForgeError::Unavailable(_))), "{r:?}");
    assert!(waited < Duration::from_secs(5), "waited {waited:?}");
}

fn reply_of_length(length: usize) -> Result<RepoAt, ForgeError> {
    let (url, served) = raw_server(move |mut s| {
        let pad = length - r#"{"data":null,"p":""}"#.len();
        let body = format!(r#"{{"data":null,"p":"{}"}}"#, "x".repeat(pad));
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
        );
    });
    let r = read_with(&url, SHORT, &token());
    served.join().unwrap();
    r
}

#[test]
fn reply_is_read_up_to_eight_mib() {
    let cap = 8 * 1024 * 1024;
    assert_eq!(reply_of_length(cap), Err(ForgeError::NotVisible));
    let over = reply_of_length(cap + 1);
    assert!(matches!(over, Err(ForgeError::Unavailable(_))), "{over:?}");
}
