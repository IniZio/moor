//! The front Proxy end to end: a lease reaches the upstream with its own user's secret and no
//! one else's; a lease of another front, an expired lease or no lease gets nothing upstream; and
//! a path the rule does not allow or a host no rule lists is refused.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::user::{Identity, UserId};

use super::{ca_dir, serve, socket_path, Deps};
use crate::proxy::nono::capture::SecretsFn;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::store::Store;
use crate::testing::bridge::bridge;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{put_secret, test_vault_key};
use crate::vault::Vault;

const ANN: &str = "ann-token-41d2";
const BOB: &str = "bob-token-8c07";

/// Raises the stop flag when dropped, so a failed assertion cannot leave `serve` running.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A rule that injects `claude` as a bearer token for the fake upstream, allowing `GET /ok`.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "claude".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "claude".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4_u32 {
            let digit = DIGITS.get(usize::try_from((n >> (18 - 6 * i)) & 63).unwrap());
            out.push(if i as usize <= chunk.len() {
                char::from(*digit.unwrap())
            } else {
                '='
            });
        }
    }
    out
}

/// A served front Proxy for the `slack` front, with Ann and Bob each holding a `claude` secret.
struct Served {
    dir: tempfile::TempDir,
    up: Upstream,
    store: Arc<Store>,
    clock: Arc<FakeClock>,
    ann: UserId,
    bob: UserId,
    port: u16,
    _stop: StopOnDrop,
}

impl Served {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let sockets = dir.path().join("s");
        let up = upstream::start(dir.path());
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let store = Arc::new(Store::open(&state.join("moor.db"), clock.clone()).unwrap());
        let vault = Vault::load(&test_vault_key(dir.path())).unwrap();
        let enroll = |who: &str, token: &str| {
            let (user, _) = store.enroll(&Identity::parse(who).unwrap()).unwrap();
            put_secret(&store, &vault, user, "claude", token);
            user
        };
        let (ann, bob) = (enroll("slack:T1:U1", ANN), enroll("slack:T1:U2", BOB));
        let shared = Arc::clone(&store);
        let secrets: SecretsFn = Arc::new(move |vault, user, name| {
            shared
                .user_secret(vault, user, name)
                .map_err(|e| e.to_string())
        });
        let stop = Arc::new(AtomicBool::new(false));
        let deps = Deps {
            store: Arc::clone(&store),
            secrets,
            vault: Arc::new(vault),
            state_dir: state.clone(),
            sockets: sockets.clone(),
            owner: rustix::process::getuid().as_raw(),
            upstream_ca: Some(up.ca_path.clone()),
            stop: Arc::clone(&stop),
            stderr: Arc::new(Mutex::new(std::io::sink())),
        };
        let fronts = vec![("slack".to_owned(), vec![rule(up.port)])];
        std::thread::spawn(move || serve(&deps, &fronts));
        let socket = socket_path(&sockets, &state, "slack");
        for _ in 0..400 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let port = bridge(&socket);
        Self {
            dir,
            up,
            store,
            clock,
            ann,
            bob,
            port,
            _stop: StopOnDrop(stop),
        }
    }

    /// A token leased to `user` on `front` for a minute.
    fn lease(&self, front: &str, user: UserId, token: &str) -> String {
        let expires = self.clock.now().saturating_add(Duration::from_secs(60));
        self.store
            .add_front_lease(token.as_bytes(), front, user, expires)
            .unwrap();
        token.to_owned()
    }

    /// The status of `GET path` on the fake upstream with `token` as the proxy password, or the
    /// error that ended it.
    fn get(&self, token: &str, port: u16, path: &str) -> Result<u16, String> {
        let ca =
            std::fs::read_to_string(ca_dir(&self.dir.path().join("state")).join("ca.pem")).unwrap();
        let agent = upstream::client(self.port, token, &ca);
        agent
            .get(format!("https://localhost:{port}{path}"))
            .call()
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    }

    /// The status line of a CONNECT to `authority` with `token` as the proxy password.
    fn connect(&self, token: &str, authority: &str) -> String {
        use std::io::{BufRead as _, BufReader, Write as _};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let auth = base64(format!("lease:{token}").as_bytes());
        write!(
            s,
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        )
        .unwrap();
        let mut line = String::new();
        let _ = BufReader::new(s).read_line(&mut line);
        line.trim_end().to_owned()
    }

    fn authorizations(&self) -> Vec<Option<String>> {
        self.up
            .seen()
            .into_iter()
            .map(|s| s.authorization)
            .collect()
    }
}

#[test]
fn each_lease_carries_only_its_own_users_secret() {
    let served = Served::start();
    let ann = served.lease("slack", served.ann, "lease-ann");
    let bob = served.lease("slack", served.bob, "lease-bob");
    assert_eq!(served.get(&ann, served.up.port, "/ok"), Ok(200));
    assert_eq!(served.get(&bob, served.up.port, "/ok"), Ok(200));
    assert_eq!(
        served.authorizations(),
        [Some(format!("Bearer {ANN}")), Some(format!("Bearer {BOB}"))]
    );
}

#[test]
fn another_fronts_lease_an_expired_lease_or_none_reaches_nothing() {
    let served = Served::start();
    let web = served.lease("web", served.ann, "lease-web");
    assert!(
        served.get(&web, served.up.port, "/ok").is_err(),
        "another front's lease"
    );
    assert!(
        served.get("no-such-lease", served.up.port, "/ok").is_err(),
        "no lease"
    );
    let ann = served.lease("slack", served.ann, "lease-ann");
    served.clock.sleep(Duration::from_secs(61));
    assert!(
        served.get(&ann, served.up.port, "/ok").is_err(),
        "an expired lease"
    );
    assert!(served.up.seen().is_empty(), "{:?}", served.up.seen());
}

#[test]
fn a_path_the_rule_does_not_allow_is_refused_and_reaches_nothing() {
    let served = Served::start();
    let ann = served.lease("slack", served.ann, "lease-ann");
    assert_eq!(served.get(&ann, served.up.port, "/nope"), Ok(403));
    assert!(served.up.seen().is_empty(), "{:?}", served.up.seen());
}

#[test]
fn a_connect_to_a_host_no_rule_lists_is_refused() {
    let served = Served::start();
    let ann = served.lease("slack", served.ann, "lease-ann");
    let line = served.connect(&ann, "unlisted.invalid:443");
    assert!(line.starts_with("HTTP/1.1 403"), "{line}");
}
