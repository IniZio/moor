//! An OIDC issuer on loopback: discovery, JWKS, userinfo, and an authorize endpoint that
//! approves at once and a PKCE token endpoint whose access and id tokens carry [`Issuer::id_claims`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_panics_doc,
    clippy::disallowed_methods,
    reason = "a fixture fails the test loudly"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use ring::digest::{digest, SHA256};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};

/// A running issuer; its thread lives as long as the test process.
#[derive(Debug, Clone)]
pub struct Issuer {
    /// `http://127.0.0.1:<port>`, also the `iss` it signs.
    pub base: String,
    /// The audience of the id tokens it issues.
    pub client_id: String,
    /// The ES256 signing key, PKCS#8.
    pub key: Vec<u8>,
    /// Claims each issued id token carries beside `iss`, `aud` and `exp`.
    pub id_claims: Arc<Mutex<Value>>,
    /// The userinfo answer: status and body.
    pub userinfo: Arc<Mutex<(u16, Value)>>,
    /// Userinfo requests that carried a bearer token.
    pub userinfo_calls: Arc<AtomicUsize>,
}

/// Unix seconds now.
#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl Issuer {
    /// Starts an issuer whose id tokens are for `client_id` and name a verified
    /// `alice@example.com`.
    #[must_use]
    pub fn start(client_id: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let rng = SystemRandom::new();
        let key = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .unwrap()
            .as_ref()
            .to_vec();
        let issuer = Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            client_id: client_id.to_owned(),
            key,
            id_claims: Arc::new(Mutex::new(
                json!({"sub": "alice-sub", "email": "alice@example.com", "email_verified": true}),
            )),
            userinfo: Arc::new(Mutex::new((
                200,
                json!({"sub": "alice-sub", "email": "Alice@Example.com", "email_verified": true}),
            ))),
            userinfo_calls: Arc::new(AtomicUsize::new(0)),
        };
        let served = issuer.clone();
        std::thread::spawn(move || {
            let mut codes = HashMap::new();
            for stream in listener.incoming().flatten() {
                served.answer(stream, &mut codes);
            }
        });
        issuer
    }

    /// `claims` signed with the issuer's key.
    #[must_use]
    pub fn sign(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_owned());
        encode(&header, claims, &EncodingKey::from_ec_der(&self.key)).unwrap()
    }

    /// The issuer's public key set.
    fn jwks(&self) -> Value {
        let rng = SystemRandom::new();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &self.key, &rng).unwrap();
        let point = pair.public_key().as_ref();
        json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]), "y": URL_SAFE_NO_PAD.encode(&point[33..65])}]})
    }

    /// Answers one request; `codes` maps each issued code to its PKCE challenge.
    fn answer(&self, stream: TcpStream, codes: &mut HashMap<String, String>) {
        let mut reader = BufReader::new(stream);
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            return;
        }
        let (mut bearer, mut length) = (false, 0);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 2 {
            let lower = line.to_ascii_lowercase();
            bearer |= lower.starts_with("authorization: bearer ");
            if let Some(n) = lower.strip_prefix("content-length:") {
                length = n.trim().parse().unwrap_or(0);
            }
            line.clear();
        }
        let mut body = vec![0; length];
        let _ = reader.read_exact(&mut body);
        let target = first.split_whitespace().nth(1).unwrap_or_default();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let query = form(query.as_bytes());
        let base = &self.base;
        let (status, extra, body) = match path {
            "/jwks" => (200, String::new(), self.jwks()),
            "/userinfo" => {
                if bearer {
                    self.userinfo_calls.fetch_add(1, Ordering::SeqCst);
                }
                let (status, body) = self.userinfo.lock().unwrap().clone();
                (status, String::new(), body)
            }
            "/authorize" => {
                let code = format!("code-{}", codes.len());
                codes.insert(code.clone(), query["code_challenge"].clone());
                let back = format!(
                    "{}?code={code}&state={}",
                    query["redirect_uri"], query["state"]
                );
                (302, format!("Location: {back}\r\n"), json!({}))
            }
            "/token" => self.token(&form(&body), codes),
            _ => (
                200,
                String::new(),
                json!({"issuer": base, "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"), "jwks_uri": format!("{base}/jwks"),
                    "userinfo_endpoint": format!("{base}/userinfo"),
                    "code_challenge_methods_supported": ["S256"]}),
            ),
        };
        let body = body.to_string();
        let mut stream = reader.into_inner();
        let _ = write!(stream, "HTTP/1.1 {status} X\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }

    /// Trades a code whose verifier matches its challenge for tokens.
    fn token(
        &self,
        form: &HashMap<String, String>,
        codes: &mut HashMap<String, String>,
    ) -> (u16, String, Value) {
        let challenge = codes.remove(form.get("code").map_or("", String::as_str));
        let verifier = form.get("code_verifier").map_or("", String::as_str);
        let matches = challenge.is_some_and(|c| {
            URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()).as_ref()) == c
        });
        if !matches || form.get("client_id") != Some(&self.client_id) {
            return (400, String::new(), json!({"error": "invalid_grant"}));
        }
        let mut claims = self.id_claims.lock().unwrap().clone();
        claims["iss"] = json!(self.base);
        claims["aud"] = json!(self.client_id);
        claims["exp"] = json!(now() + 300);
        let token = self.sign(&claims);
        let answer = json!({"access_token": token, "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "refresh-0", "id_token": token});
        (200, String::new(), answer)
    }
}

/// A form-encoded string as a map.
fn form(bytes: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(bytes).into_owned().collect()
}
