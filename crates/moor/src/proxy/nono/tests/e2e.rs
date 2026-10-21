//! The embedded proxy end to end on loopback against a fake HTTPS upstream.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, Secret};
use tempfile::TempDir;
use ureq::Agent;

use super::upstream::{self, Upstream};
use crate::proxy::nono::audit::{self, BoxSink, EgressSink, Refusal, Withheld};
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::proxy::ProxyLog;
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// The credential every rule fetches.
const SECRET: &str = "s3cr3t-credential-value-7f3a";
/// The session token nono checks, sent as the proxy password.
const TOKEN: &str = "token_AbCdEf-0123456789";
/// What the client sends before the proxy swaps it out.
const PLACEHOLDER: &str = "Bearer placeholder-not-real";
/// Box id whose CA directory the proxy uses.
const BOX_ID: BoxId = BoxId::new(7);

/// Counts fetches per rule and lets a test make them fail.
struct Fetches {
    /// Fetch count of the header rule.
    header: Arc<AtomicUsize>,
    /// Fetch count of the basic-auth rule.
    basic: Arc<AtomicUsize>,
    /// When set, fetches fail.
    fail: Arc<AtomicBool>,
}

/// Everything one test needs.
pub(super) struct Rig {
    /// Owns every file.
    dir: TempDir,
    /// The fake upstream behind the header rule.
    pub(super) up: Upstream,
    /// The fake upstream behind the basic-auth rule.
    up2: Upstream,
    /// Fetch counters.
    fetches: Fetches,
    /// The capture backend handed to nono.
    pub(super) capture: Arc<MoorCapture>,
    /// Source of time for the capture.
    clock: Arc<FakeClock>,
    /// The Box CA.
    ca: ca::BoxCa,
    /// Running proxy.
    proxy: Option<NonoProxy>,
    /// Session token nono checks.
    token: Secret,
}

impl Rig {
    /// State dir under the temp dir.
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    /// The running proxy.
    pub(super) fn proxy(&self) -> &NonoProxy {
        self.proxy.as_ref().unwrap()
    }

    /// A client through the running proxy trusting the Box CA.
    pub(super) fn client(&self) -> Agent {
        client(self.proxy().port(), self.ca.cert_pem())
    }

    /// `https://localhost:<header upstream port><path>`.
    pub(super) fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.up.port)
    }

    /// A store with one Box, and a proxy log for it.
    pub(super) fn store(&self) -> (Store, BoxId, ProxyLog) {
        let store = Store::open(&self.state().join("moor.db"), self.clock.clone()).unwrap();
        let box_id = store
            .apply(Decision::insert(NewBox {
                name: BoxName::parse_allow_bare("t").unwrap(),
                repo: "r".into(),
            }))
            .unwrap()
            .box_id;
        let log = ProxyLog::open(&self.state(), box_id, self.clock.clone()).unwrap();
        (store, box_id, log)
    }
}

/// Header rule on `localhost:<port>` and basic-auth rule on `localhost:<port2>`.
fn rules(port: u16, port2: u16) -> Vec<ResolvedRule> {
    let rule = |name: &str, host: String, inject| ResolvedRule {
        name: name.to_owned(),
        hosts: vec![host],
        credential: "TOKEN".to_owned(),
        inject,
        endpoint_rules: vec![EndpointRule {
            method: "*".to_owned(),
            path: "/**".to_owned(),
        }],
        box_env: BTreeMap::new(),
    };
    vec![
        rule(
            "hdr",
            format!("localhost:{port}"),
            ResolvedInject::Header {
                header: "authorization".to_owned(),
                form: "Bearer {secret}".to_owned(),
            },
        ),
        rule(
            "basic",
            format!("localhost:{port2}"),
            ResolvedInject::BasicAuth {
                user: "x-access-token".to_owned(),
            },
        ),
    ]
}

/// A source whose fetch counts calls and can be made to fail.
fn source(count: &Arc<AtomicUsize>, fail: &Arc<AtomicBool>, user: Option<&str>) -> RuleSource {
    let (count, fail) = (Arc::clone(count), Arc::clone(fail));
    RuleSource {
        config_key: "credentials.TOKEN".to_owned(),
        fetch: Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            if fail.load(Ordering::SeqCst) {
                Err("credentials.TOKEN unavailable".to_owned())
            } else {
                Ok(Secret::new(SECRET.as_bytes().to_vec()))
            }
        }),
        basic_user: user.map(str::to_owned),
    }
}

/// Starts a proxy for `rules` over `up`, with the given Box CA and capture.
fn start_proxy(
    dir: &Path,
    token: &Secret,
    ca: &ca::BoxCa,
    ups: [&Upstream; 2],
    capture: &Arc<MoorCapture>,
) -> NonoProxy {
    let ca_dir = ca::ca_dir(&dir.join("state"), BOX_ID);
    let rules = rules(ups[0].port, ups[1].port);
    let cfg = build_config(&NonoSpec {
        rules: &rules,
        token,
        ca,
        ca_dir: &ca_dir,
        upstream_ca: Some(&ups[0].ca_path),
        chain: None,
    });
    NonoProxy::start(cfg, Arc::clone(capture)).unwrap()
}

/// Builds a rig with both rules and a running proxy.
pub(super) fn rig() -> Rig {
    let dir = TempDir::new().unwrap();
    let up = upstream::start(dir.path());
    let up2 = up.twin();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let fetches = Fetches {
        header: Arc::new(AtomicUsize::new(0)),
        basic: Arc::new(AtomicUsize::new(0)),
        fail: Arc::new(AtomicBool::new(false)),
    };
    let sources = BTreeMap::from([
        (
            "hdr".to_owned(),
            source(&fetches.header, &fetches.fail, None),
        ),
        (
            "basic".to_owned(),
            source(&fetches.basic, &fetches.fail, Some("x-access-token")),
        ),
    ]);
    let capture = Arc::new(MoorCapture::new(sources));
    let ca = ca::load_or_create(&dir.path().join("state"), BOX_ID).unwrap();
    let token = Secret::new(TOKEN.as_bytes().to_vec());
    let proxy = start_proxy(dir.path(), &token, &ca, [&up, &up2], &capture);
    Rig {
        dir,
        up,
        up2,
        fetches,
        capture,
        clock,
        ca,
        proxy: Some(proxy),
        token,
    }
}

/// A client that goes through the proxy and trusts only `ca_pem`.
fn client(proxy_port: u16, ca_pem: &str) -> Agent {
    upstream::client(proxy_port, TOKEN, ca_pem)
}

/// GETs `url` with the placeholder credential; status or error text.
pub(super) fn get(agent: &Agent, url: &str) -> Result<u16, String> {
    agent
        .get(url)
        .header("Authorization", PLACEHOLDER)
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}

/// Sink that records calls as text.
#[derive(Default)]
pub(super) struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    /// Recorded calls.
    pub(super) fn calls(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Appends one call.
    fn push(&self, s: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(s);
    }
}

impl EgressSink for Recorder {
    fn allowed(&self, method: &str, route: &str, status: u16) {
        self.push(format!("allowed({method}, {route}, {status})"));
    }

    fn refused(&self, r: &Refusal<'_>) {
        self.push(format!("refused({}, {:?})", r.host, r.rule));
    }

    fn aborted(&self, host: &str, rule: Option<&str>) {
        self.push(format!("aborted({host}, {rule:?})"));
    }

    fn credential_failed(&self, rule: &str) {
        self.push(format!("credential_failed({rule})"));
    }

    fn withheld(&self, host: &str, rule: &str, method: &str, _: Option<&str>) {
        self.push(format!("withheld({host}, {rule}, {method})"));
    }

    fn withheld_overflow(&self) {
        self.push("withheld_overflow".to_owned());
    }
}

/// Drains and applies audit events until `done` holds for the recorded calls.
fn settle(rig: &Rig, sink: &Recorder, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    for _ in 0..100 {
        audit::apply(&rig.proxy().drain_audit(), &Withheld::default(), sink);
        if done(&sink.calls()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    sink.calls()
}

/// Event kinds of a Box.
pub(super) fn kinds(store: &Store, box_id: BoxId) -> Vec<String> {
    store
        .events(box_id, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// Standard-alphabet base64 decoder for assertions.
fn decode_b64(s: &str) -> String {
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut bits, mut n, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = table.iter().position(|t| *t == c).unwrap();
        bits = (bits << 6) | u32::try_from(v).unwrap();
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push(u8::try_from((bits >> n) & 0xff).unwrap());
        }
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn header_rule_swaps_placeholder_for_secret() {
    let rig = rig();
    assert_eq!(get(&rig.client(), &rig.url("/ok")), Ok(200));
    let seen = rig.up.seen();
    let last = seen.last().unwrap();
    assert_eq!(last.line, "GET /ok HTTP/1.1");
    assert_eq!(
        last.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[test]
fn basic_rule_sends_user_and_secret_as_basic_auth() {
    let rig = rig();
    let url = format!("https://localhost:{}/ok", rig.up2.port);
    assert_eq!(get(&rig.client(), &url), Ok(200));
    let got = rig
        .up2
        .seen()
        .last()
        .unwrap()
        .authorization
        .clone()
        .unwrap();
    let encoded = got.strip_prefix("Basic ").unwrap();
    assert_eq!(decode_b64(encoded), format!("x-access-token:{SECRET}"));
    assert_eq!(rig.fetches.basic.load(Ordering::SeqCst), 1);
}

#[test]
fn unlisted_host_is_refused_and_reaches_the_sinks() {
    let rig = rig();
    let before = rig.up.seen().len();
    let res = get(&rig.client(), "https://example.invalid/anything");
    assert!(res.is_err() || res == Ok(403), "got {res:?}");
    assert_eq!(rig.up.seen().len(), before);

    let sink = Recorder::default();
    let calls = settle(&rig, &sink, |c| c.iter().any(|l| l.starts_with("refused(")));
    assert!(
        calls
            .iter()
            .any(|l| l.starts_with("refused(example.invalid")),
        "{calls:?}"
    );

    let (store, box_id, log) = rig.store();
    let box_sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let again = rig.client();
    assert!(get(&again, "https://example.invalid/anything").is_err());
    for _ in 0..100 {
        audit::apply(&rig.proxy().drain_audit(), &Withheld::default(), &box_sink);
        if kinds(&store, box_id).iter().any(|k| k == "egress_refused") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let found = kinds(&store, box_id);
    assert!(found.iter().any(|k| k == "egress_refused"), "{found:?}");
}

#[test]
fn box_ca_is_reused_across_restart_and_key_is_private() {
    let mut rig = rig();
    let old_pem = rig.ca.cert_pem().to_owned();
    rig.proxy.take().unwrap().stop();
    let again = ca::load_or_create(&rig.state(), BOX_ID).unwrap();
    assert_eq!(again.cert_pem(), old_pem);
    let key = ca::ca_dir(&rig.state(), BOX_ID).join("ca-key.pem");
    let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let proxy = start_proxy(
        rig.dir.path(),
        &rig.token,
        &again,
        [&rig.up, &rig.up2],
        &rig.capture,
    );
    let agent = client(proxy.port(), &old_pem);
    assert_eq!(get(&agent, &rig.url("/ok")), Ok(200));
    proxy.stop();
}

#[test]
fn credential_is_read_again_for_every_request() {
    let rig = rig();
    let agent = rig.client();
    for n in 1..=3 {
        assert_eq!(get(&agent, &rig.url("/ok")), Ok(200));
        assert_eq!(rig.fetches.header.load(Ordering::SeqCst), n);
    }
}

#[test]
fn fetch_failure_is_reported_and_never_reaches_upstream() {
    let rig = rig();
    rig.fetches.fail.store(true, Ordering::SeqCst);
    let agent = rig.client();
    let before = rig.up.seen().len();
    let res = get(&agent, &rig.url("/ok"));
    assert_ne!(res, Ok(200), "{res:?}");
    assert_eq!(rig.up.seen().len(), before);

    let sink = Recorder::default();
    let calls = settle(&rig, &sink, |c| {
        c.iter().any(|l| l.starts_with("credential_failed("))
    });
    assert!(
        calls.contains(&"credential_failed(hdr)".to_owned()),
        "{calls:?}"
    );

    let (store, box_id, log) = rig.store();
    let box_sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    box_sink.credential_failed("hdr");
    assert!(
        kinds(&store, box_id)
            .iter()
            .any(|k| k == "proxy_credential_failed"),
        "{:?}",
        kinds(&store, box_id)
    );

    rig.fetches.fail.store(false, Ordering::SeqCst);
    assert_eq!(get(&agent, &rig.url("/ok")), Ok(200));
}

#[test]
fn secret_bytes_never_appear_in_logs_debug_or_errors() {
    let rig = rig();
    let agent = rig.client();
    let (store, box_id, log) = rig.store();
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let mut texts = vec![
        format!("{:?}", rig.proxy()),
        format!("{:?}", rig.capture),
        format!("{:?}", rig.ca),
    ];
    assert_eq!(get(&agent, &rig.url("/ok")), Ok(200));
    texts.push(format!("{:?}", get(&agent, "https://example.invalid/x")));
    rig.fetches.fail.store(true, Ordering::SeqCst);
    texts.push(format!("{:?}", get(&agent, &rig.url("/ok"))));

    for _ in 0..40 {
        audit::apply(&rig.proxy().drain_audit(), &Withheld::default(), &sink);
        std::thread::sleep(Duration::from_millis(25));
    }
    let log_path = rig
        .state()
        .join("proxy")
        .join(format!("{}.log", box_id.get()));
    texts.push(std::fs::read_to_string(log_path).unwrap());
    for e in store.events(box_id, 100).unwrap() {
        texts.push(format!("{e:?}"));
    }
    let rules = rules(rig.up.port, rig.up2.port);
    let cfg = build_config(&NonoSpec {
        rules: &rules,
        token: &rig.token,
        ca: &rig.ca,
        ca_dir: &ca::ca_dir(&rig.state(), BOX_ID),
        upstream_ca: None,
        chain: None,
    });
    texts.push(format!("{cfg:?}"));
    for t in &texts {
        assert!(!t.contains(SECRET), "secret leaked: {t}");
        assert!(!t.contains(TOKEN), "token leaked: {t}");
    }
}
