//! The ledger's TLS listener: it answers the shared egress Proxy that checks the controller's
//! certificate, presents a client certificate from the ledger CA and the egress key, serves a
//! machine runner presenting a client certificate from that CA and the service key as the
//! runner socket does, serves a remote front presenting its own key as that front's socket does,
//! refuses anything else, and stays up through a drain.
#![allow(clippy::panic, reason = "test assertions")]
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_client::tls::TlsPki;
use moor_client::{Client, ClientError};
use moor_core::request::Request;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::Value;

use super::{config, env_with, Buf};
use crate::controller::drain::Drain;
use crate::controller::{start, Running, Sink};
use crate::proxy::ledger::{EgressLedger, Endpoint, LedgerError, TlsFiles};

const KEY: &str = "egress-key";
const SERVICE_KEY: &str = "service-key";

/// Writes a CA as `<name>-ca.pem` and, when `leaf` is set, a certificate it signed for
/// 127.0.0.1 as `<name>.pem` with its key as `<name>.key`, and a client certificate as
/// `<name>-client.pem` with its key as `<name>-client.key`; returns the CA's path.
fn pki(dir: &Path, name: &str, leaf: bool) -> PathBuf {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca.distinguished_name
        .push(DnType::CommonName, format!("{name} CA"));
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let ca_path = dir.join(format!("{name}-ca.pem"));
    std::fs::write(&ca_path, ca_cert.pem()).unwrap();
    if leaf {
        let issuer = Issuer::new(ca, ca_key);
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::IpAddress([127, 0, 0, 1].into())];
        let cert = params.signed_by(&key, &issuer).unwrap();
        std::fs::write(dir.join(format!("{name}.pem")), cert.pem()).unwrap();
        std::fs::write(dir.join(format!("{name}.key")), key.serialize_pem()).unwrap();
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::default()
            .signed_by(&key, &issuer)
            .unwrap();
        std::fs::write(dir.join(format!("{name}-client.pem")), cert.pem()).unwrap();
        std::fs::write(dir.join(format!("{name}-client.key")), key.serialize_pem()).unwrap();
    }
    ca_path
}

/// The config of a controller serving the ledger on TLS at 127.0.0.1 with `pki`'s `ctl` files,
/// after `extra`.
fn tls_config(dir: &Path, extra: &str) -> PathBuf {
    let (cert, key, ca) = (
        dir.join("ctl.pem"),
        dir.join("ctl.key"),
        dir.join("ctl-ca.pem"),
    );
    config(
        dir,
        &format!(
            "{extra}ledger_tls:\n  listen: 127.0.0.1:0\n  cert: {}\n  key: {}\n  client_ca: {}\n",
            cert.display(),
            key.display(),
            ca.display()
        ),
    )
}

/// A started controller with the TLS ledger on; its address and the CA that signed it.
fn started(dir: &tempfile::TempDir, extra: &str) -> (Running, SocketAddr, PathBuf) {
    started_with(dir, extra, &[("MOOR_EGRESS_KEY", KEY)])
}

/// [`started`] with `keys` in the controller's environment.
fn started_with(
    dir: &tempfile::TempDir,
    extra: &str,
    keys: &[(&str, &str)],
) -> (Running, SocketAddr, PathBuf) {
    let ca = pki(dir.path(), "ctl", true);
    let cfg = tls_config(dir.path(), extra);
    let running = start(&env_with(dir.path(), keys), &cfg).expect("start");
    let addr = running
        .ledger_tls
        .as_ref()
        .expect("TLS ledger")
        .listener
        .local_addr()
        .unwrap();
    (running, addr, ca)
}

/// The TLS ledger at `addr`, checked against `ca`, dialled with the client certificate `pki`
/// wrote for `client`.
fn endpoint_as(addr: SocketAddr, ca: &Path, client: &str) -> Endpoint {
    let dir = ca.parent().expect("dir");
    let (cert, key) = (
        dir.join(format!("{client}-client.pem")),
        dir.join(format!("{client}-client.key")),
    );
    let files = TlsFiles {
        ca,
        cert: &cert,
        key: &key,
    };
    Endpoint::parse(&format!("tls://{addr}"), Some(files)).expect("endpoint")
}

/// The TLS ledger at `addr`, checked against `ca`, dialled with the controller CA's client
/// certificate.
fn endpoint(addr: SocketAddr, ca: &Path) -> Endpoint {
    endpoint_as(addr, ca, "ctl")
}

/// The first line the controller answers `hello` with over TLS.
fn say(at: &Endpoint, hello: &str) -> String {
    let mut stream = BufReader::new(at.dial(Duration::from_secs(5)).expect("dial"));
    writeln!(stream.get_mut(), "{hello}").unwrap();
    stream.get_mut().flush().unwrap();
    let mut line = String::new();
    stream.read_line(&mut line).unwrap();
    line
}

fn serve(running: Running) {
    running.serve_in_background(Arc::new(Mutex::new(Buf::default())));
}

#[test]
fn the_egress_proxy_with_the_ca_and_key_is_answered_over_tls() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "");
    serve(running);
    let ledger = EgressLedger::at(endpoint(addr, &ca), KEY.to_owned());
    assert_eq!(ledger.token_box("no-such-token"), Ok(None));
    let awake = ledger.awake_tokens();
    assert!(awake.is_ok(), "the warm pass is answered: {awake:?}");
}

#[test]
fn a_connect_without_a_client_certificate_is_refused_in_the_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "");
    serve(running);
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(&ca).unwrap() {
        roots.add(cert.unwrap()).unwrap();
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let conn = ClientConnection::new(Arc::new(config), ServerName::try_from("127.0.0.1").unwrap())
        .unwrap();
    let sock = TcpStream::connect(addr).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let tls = StreamOwned::new(conn, sock);
    assert!(refusal(tls).contains("CertificateRequired"));
}

#[test]
fn a_client_certificate_another_ca_signed_is_refused_in_the_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "");
    serve(running);
    pki(dir.path(), "stranger", true);
    let at = endpoint_as(addr, &ca, "stranger");
    assert!(refusal(at.dial(Duration::from_secs(5)).expect("dial")).contains("UnknownCA"));
}

/// The alert the controller ends the handshake with, read before the client says anything so
/// no write races the close.
fn refusal(mut tls: impl std::io::Read) -> String {
    let mut byte = [0; 1];
    match tls.read(&mut byte) {
        Err(e) => e.to_string(),
        Ok(n) => panic!("the controller kept the connection and sent {n} bytes"),
    }
}

#[test]
fn a_wrong_egress_key_over_tls_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "");
    serve(running);
    let ledger = EgressLedger::at(endpoint(addr, &ca), "not-the-key".to_owned());
    assert!(matches!(
        ledger.token_box("no-such-token"),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn a_controller_certificate_the_ca_did_not_sign_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, _) = started(&dir, "");
    serve(running);
    let stranger = pki(dir.path(), "stranger", false);
    let ledger = EgressLedger::at(endpoint(addr, &stranger), KEY.to_owned());
    match ledger.token_box("no-such-token") {
        Err(LedgerError::Unavailable(why)) => assert!(why.contains("certificate"), "{why}"),
        other => panic!("answered an untrusted controller: {other:?}"),
    }
}

#[test]
fn a_connect_without_tls_gets_no_ledger_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, _) = started(&dir, "");
    serve(running);
    let mut plain = TcpStream::connect(addr).unwrap();
    plain
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        plain,
        r#"{{"egress":{{"v":1,"key":"{KEY}","box_id":null}}}}"#
    )
    .unwrap();
    let mut reply = Vec::new();
    drop(plain.read_to_end(&mut reply));
    assert!(
        !String::from_utf8_lossy(&reply).contains("Ok"),
        "a plaintext hello was answered: {reply:?}"
    );
}

#[test]
fn old_supervisors_and_the_front_proxy_are_refused_over_tls() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "fronts:\n  slack:\n    proxy_rules: [claude]\n");
    let fronts_key = running
        .ledger
        .1
        .as_deref()
        .expect("front Proxy key")
        .to_owned();
    serve(running);
    let at = endpoint(addr, &ca);
    let front = say(
        &at,
        &format!(r#"{{"fronts":{{"v":1,"key":"{fronts_key}"}}}}"#),
    );
    assert!(front.contains("refused"), "{front}");
    let supervisor = say(&at, r#"{"box":{"v":1,"box_id":1,"key":"k"}}"#);
    assert!(supervisor.contains("bad hello"), "{supervisor}");
}

#[test]
fn the_tls_ledger_keeps_answering_while_the_controller_drains() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "drain_timeout_secs: 30\n");
    let drain = Arc::new(Drain::default());
    let in_flight = drain.hold().expect("held");
    let (wake, signal) = UnixStream::pair().expect("pair");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    let serving = Arc::clone(&drain);
    let served = std::thread::spawn(move || running.serve_until(&wake, &serving, &sink));
    let pooled = EgressLedger::at(endpoint(addr, &ca), KEY.to_owned());
    assert_eq!(pooled.token_box("x"), Ok(None));

    (&signal).write_all(&[1]).expect("wake");
    while !drain.begun() {
        std::thread::yield_now();
    }
    let fresh = EgressLedger::at(endpoint(addr, &ca), KEY.to_owned());
    assert_eq!(
        fresh.token_box("x"),
        Ok(None),
        "a new connection is answered mid-drain"
    );
    assert_eq!(
        pooled.token_box("x"),
        Ok(None),
        "a pooled connection is answered mid-drain"
    );
    drop(in_flight);
    served.join().expect("drained");
}

#[test]
fn the_tls_ledger_needs_the_egress_key_a_readable_certificate_and_client_ca() {
    let dir = tempfile::tempdir().unwrap();
    pki(dir.path(), "ctl", true);
    let cfg = tls_config(dir.path(), "");
    let Err(stop) = start(&env_with(dir.path(), &[("MOOR_EGRESS_KEY", "")]), &cfg) else {
        panic!("started a TLS ledger without the egress key");
    };
    assert!(stop.message.contains("MOOR_EGRESS_KEY"), "{}", stop.message);

    std::fs::write(dir.path().join("ctl-ca.pem"), "").unwrap();
    let Err(stop) = start(&env_with(dir.path(), &[("MOOR_EGRESS_KEY", KEY)]), &cfg) else {
        panic!("started a TLS ledger with an empty client CA");
    };
    assert!(
        stop.message.contains("ledger_tls.client_ca"),
        "{}",
        stop.message
    );
    assert!(stop.message.contains("no certificate"), "{}", stop.message);

    std::fs::remove_file(dir.path().join("ctl-ca.pem")).unwrap();
    let Err(stop) = start(&env_with(dir.path(), &[("MOOR_EGRESS_KEY", KEY)]), &cfg) else {
        panic!("started a TLS ledger without its client CA");
    };
    assert!(
        stop.message.contains("ledger_tls.client_ca"),
        "{}",
        stop.message
    );

    std::fs::remove_file(dir.path().join("ctl.pem")).unwrap();
    let Err(stop) = start(&env_with(dir.path(), &[("MOOR_EGRESS_KEY", KEY)]), &cfg) else {
        panic!("started a TLS ledger without its certificate");
    };
    assert!(stop.message.contains("ledger_tls.cert"), "{}", stop.message);
}

/// A served controller taking both the egress Proxy and runners on TLS; its address and CA.
fn serving_runners(dir: &tempfile::TempDir) -> (SocketAddr, PathBuf) {
    let keys = [
        ("MOOR_EGRESS_KEY", KEY),
        ("MOOR_MACHINE_SERVICE_KEY", SERVICE_KEY),
    ];
    let (running, addr, ca) = started_with(dir, "", &keys);
    serve(running);
    (addr, ca)
}

/// What a runner presenting `key` gets for `request`, as JSON.
fn runner_asks(
    addr: SocketAddr,
    ca: &Path,
    key: &str,
    request: &str,
) -> Result<Value, ClientError> {
    let dir = ca.parent().expect("pki dir");
    let files = TlsPki {
        ca: ca.to_owned(),
        cert: dir.join("ctl-client.pem"),
        key: dir.join("ctl-client.key"),
    };
    let client = Client::runner(&format!("tls://{addr}"), &files, key).expect("client");
    let request: Request = serde_json::from_str(request).expect("request");
    client
        .request(request, |_| {})
        .map(|r| serde_json::to_value(r).expect("encode"))
}

#[test]
fn a_runner_with_the_service_key_gets_show_and_machine_keys_answered_over_tls() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_runners(&dir);
    for request in [r#"{"cmd":"show"}"#, r#"{"cmd":"machine-keys"}"#] {
        let got = runner_asks(addr, &ca, SERVICE_KEY, request).expect("answered");
        assert_eq!(got["class"], "ok", "{request}: {got}");
    }
}

#[test]
fn a_runner_with_a_wrong_key_is_refused_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_runners(&dir);
    match runner_asks(addr, &ca, "not-the-key", r#"{"cmd":"show"}"#) {
        Err(ClientError::Unauthorized(why)) => {
            assert!(why.contains("not the machine service key"), "{why}");
        }
        other => panic!("a wrong key was served: {other:?}"),
    }
}

#[test]
fn a_runner_over_tls_is_refused_what_the_runner_socket_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_runners(&dir);
    let got = runner_asks(addr, &ca, SERVICE_KEY, r#"{"cmd":"machine-list"}"#).expect("answered");
    assert_eq!(got["error"]["kind"], "refused", "{got}");
    let message = got["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("the machine runner's socket serves only"),
        "{got}"
    );
}

#[test]
fn the_egress_hello_is_still_answered_beside_runners() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_runners(&dir);
    let ledger = EgressLedger::at(endpoint(addr, &ca), KEY.to_owned());
    assert_eq!(ledger.token_box("no-such-token"), Ok(None));
    let runner = say(
        &endpoint(addr, &ca),
        &format!(r#"{{"runner":{{"v":1,"key":"{SERVICE_KEY}"}}}}"#),
    );
    assert!(runner.contains("Ok"), "{runner}");
}

#[test]
fn a_runner_is_refused_where_no_service_key_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let (running, addr, ca) = started(&dir, "");
    serve(running);
    match runner_asks(addr, &ca, SERVICE_KEY, r#"{"cmd":"show"}"#) {
        Err(ClientError::Unauthorized(why)) => assert!(why.contains("unset"), "{why}"),
        other => panic!("a runner was served without a service key: {other:?}"),
    }
}

mod fronts;
