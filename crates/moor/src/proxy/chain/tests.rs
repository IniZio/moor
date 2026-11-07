//! Tunnels opened through an HTTP proxy with `CONNECT`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};

use super::fake::{Answer, FakeProxy};
use super::*;

fn proxy(url: &str) -> Proxy {
    Proxy::new(url).unwrap()
}

#[test]
fn reaches_the_target_through_the_proxy() {
    let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let to = target.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut tcp, _) = target.accept().unwrap();
        let mut buf = [0u8; 4];
        tcp.read_exact(&mut buf).unwrap();
        tcp.write_all(&buf.to_ascii_uppercase()).unwrap();
    });
    let fake = FakeProxy::start(Answer::Tunnel(Some(to)));
    let mut tcp = connect(&proxy(&fake.url()), "api.github.com", 443).unwrap();
    tcp.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    tcp.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"PING");
    assert_eq!(fake.seen(), ["CONNECT api.github.com:443 HTTP/1.1"]);
}

#[test]
fn ipv6_targets_are_bracketed() {
    let fake = FakeProxy::start(Answer::Refuse);
    let _ = connect(&proxy(&fake.url()), "2001:db8::1", 443);
    assert_eq!(fake.seen(), ["CONNECT [2001:db8::1]:443 HTTP/1.1"]);
}

#[test]
fn a_refused_connect_is_an_error_naming_the_status() {
    let fake = FakeProxy::start(Answer::Refuse);
    let err = connect(&proxy(&fake.url()), "api.github.com", 443).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("403") && text.contains("api.github.com:443"),
        "{text}"
    );
}

#[test]
fn an_unreachable_proxy_is_an_error() {
    let gone = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    std::thread::spawn(move || gone.incoming().for_each(drop));
    assert!(connect(&proxy(&url), "api.github.com", 443).is_err());
}

#[test]
fn proxies_with_credentials_or_other_schemes_are_refused() {
    for url in ["http://u:p@127.0.0.1:1", "socks5://127.0.0.1:1"] {
        let err = connect(&proxy(url), "api.github.com", 443).unwrap_err();
        assert!(err.to_string().contains("not supported"), "{url}: {err}");
    }
}
