//! Signing in to a moor server: OIDC authorization code with PKCE. The browser comes back to
//! a loopback port; when it cannot, the user pastes the address it ended on.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::url::Url;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, ExtraTokenFields, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, StandardRevocableToken, StandardTokenResponse,
    TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

pub use http::Http;

mod http;

/// The loopback port the identity provider's redirect URI names.
pub const REDIRECT_PORT: u16 = 53682;

/// How long a sign-in waits for the browser or a paste.
const WAIT: Duration = Duration::from_mins(10);

/// What the server publishes at `/.well-known/moor`.
#[derive(Deserialize)]
struct Wellknown {
    /// The OIDC issuer.
    issuer: String,
    /// The public client id.
    client_id: String,
    /// The scopes to ask for.
    scopes: Vec<String>,
}

/// The parts of the issuer's OIDC discovery document a sign-in needs.
#[derive(Deserialize)]
struct Provider {
    /// Names itself; must be the issuer asked.
    issuer: String,
    /// Where the browser signs in.
    authorization_endpoint: String,
    /// Where codes and refresh tokens are traded.
    token_endpoint: String,
    /// The PKCE methods it accepts; empty when it does not say.
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

/// How a sign-in talks to the person.
pub trait Ui {
    /// Shows a line.
    fn say(&mut self, line: &str);
    /// Opens `url` in a browser, if there is one to open.
    fn open(&mut self, url: &str);
    /// Starts sending the callback addresses the person pastes to `to`, as they are entered.
    fn pasted(&mut self, to: Sender<String>);
}

/// The terminal and the platform's browser opener.
#[derive(Debug, Default)]
pub struct Console;

impl Ui for Console {
    fn say(&mut self, line: &str) {
        let _ = writeln!(std::io::stderr(), "{line}");
    }

    fn open(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    fn pasted(&mut self, to: Sender<String>) {
        let _ = std::thread::Builder::new()
            .name("moor-paste".to_owned())
            .spawn(move || {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    if !line.trim().is_empty() && to.send(line).is_err() {
                        break;
                    }
                }
            });
    }
}

/// A sign-in under way: what the browser needs and what the token request will present.
struct Pending {
    /// The address that starts the browser's part.
    url: Url,
    /// The `state` the browser must bring back.
    state: CsrfToken,
    /// The PKCE verifier.
    verifier: String,
    /// The redirect URI named in both requests.
    redirect: String,
}

/// The id token an OIDC token response carries beside the OAuth fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IdToken {
    /// Present when `openid` was granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
}

impl ExtraTokenFields for IdToken {}

/// A token response with its id token.
type Answer = StandardTokenResponse<IdToken, BasicTokenType>;

/// An OAuth client whose token responses keep the id token.
type OidcClient = oauth2::Client<
    BasicErrorResponse,
    Answer,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
>;

/// What a finished sign-in holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    /// The moor server, without a trailing slash.
    pub server: String,
    /// The OIDC issuer the server named.
    pub issuer: String,
    /// The public client id the server named.
    pub client_id: String,
    /// Where refresh tokens are traded.
    pub token_endpoint: String,
    /// The bearer token.
    pub access_token: String,
    /// How long the access token lives, when the issuer says.
    pub expires_in: Option<Duration>,
    /// The long-lived grant, when offline access was granted.
    pub refresh_token: Option<String>,
    /// The OIDC id token, when `openid` was granted.
    pub id_token: Option<String>,
}

/// Signs in to `server`. `port` is the loopback port for the browser's return; 0 takes any
/// free one.
///
/// # Errors
/// The server or its issuer cannot be reached or misbehaves, or the sign-in is refused or not
/// completed in time.
pub fn sign_in(server: &str, port: u16, ui: &mut dyn Ui) -> Result<SignedIn, String> {
    let server = server.trim_end_matches('/').to_owned();
    let http = Http::new();
    let found: Wellknown = http.get_json(&format!("{server}/.well-known/moor"))?;
    let provider = discover(&http, &found.issuer)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok();
    let port = listener
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map_or(port, |a| a.port());
    let pending = authorize(&found, &provider, port)?;
    let done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    if let Some(listener) = listener {
        listen(listener, &pending.redirect, &tx, &done);
    }
    ui.pasted(tx);
    ui.say(&format!(
        "Sign in to {server} in your browser:\n{}",
        pending.url
    ));
    ui.say(&format!(
        "If the browser cannot reach localhost:{port} on this machine, paste the address it ends on here."
    ));
    ui.open(pending.url.as_str());
    let code = wait_for_code(&rx, ui, &pending.state, &provider.issuer);
    done.store(true, Ordering::Relaxed);
    let token = exchange(&http, (&found, &provider), &pending, &code?)?;
    Ok(SignedIn {
        server,
        issuer: provider.issuer,
        client_id: found.client_id,
        token_endpoint: provider.token_endpoint,
        access_token: token.access_token().secret().clone(),
        expires_in: token.expires_in(),
        refresh_token: token.refresh_token().map(|r| r.secret().clone()),
        id_token: token.extra_fields().id_token.clone(),
    })
}

/// The browser's address and the secrets that go with it.
fn authorize(found: &Wellknown, provider: &Provider, port: u16) -> Result<Pending, String> {
    let redirect = format!("http://localhost:{port}/callback");
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let auth = AuthUrl::new(provider.authorization_endpoint.clone()).map_err(|e| e.to_string())?;
    let (url, state) = OidcClient::new(ClientId::new(found.client_id.clone()))
        .set_auth_uri(auth)
        .set_redirect_uri(RedirectUrl::new(redirect.clone()).map_err(|e| e.to_string())?)
        .authorize_url(CsrfToken::new_random)
        .add_scopes(found.scopes.iter().cloned().map(Scope::new))
        .set_pkce_challenge(challenge)
        .url();
    Ok(Pending {
        url,
        state,
        verifier: verifier.into_secret(),
        redirect,
    })
}

/// Trades `code` for tokens.
fn exchange(
    http: &Http,
    (found, provider): (&Wellknown, &Provider),
    pending: &Pending,
    code: &str,
) -> Result<Answer, String> {
    let token_url = TokenUrl::new(provider.token_endpoint.clone()).map_err(|e| e.to_string())?;
    OidcClient::new(ClientId::new(found.client_id.clone()))
        .set_token_uri(token_url)
        .set_redirect_uri(RedirectUrl::new(pending.redirect.clone()).map_err(|e| e.to_string())?)
        .exchange_code(AuthorizationCode::new(code.to_owned()))
        .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier.clone()))
        .request(http)
        .map_err(|e| {
            format!(
                "the token request to {} failed: {e}",
                provider.token_endpoint
            )
        })
}

/// The issuer's OIDC discovery document, which must name the issuer asked.
fn discover(http: &Http, issuer: &str) -> Result<Provider, String> {
    let at = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let provider: Provider = http.get_json(&at)?;
    if provider.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(format!("{at} names {}, not {issuer}", provider.issuer));
    }
    let methods = &provider.code_challenge_methods_supported;
    if !methods.is_empty() && !methods.iter().any(|m| m == "S256") {
        return Err(format!("{issuer} does not offer PKCE S256"));
    }
    Ok(provider)
}

/// Waits for the first callback address that carries a code for this sign-in, from the
/// loopback or pasted. An address that does not fit is reported and skipped.
fn wait_for_code(
    rx: &Receiver<String>,
    ui: &mut dyn Ui,
    state: &CsrfToken,
    issuer: &str,
) -> Result<String, String> {
    loop {
        let got = rx
            .recv_timeout(WAIT)
            .map_err(|_| "timed out waiting for the browser".to_owned())?;
        match code_in(&got, state, issuer) {
            Ok(code) => return Ok(code),
            Err(Callback::Refused(why)) => return Err(why),
            Err(Callback::Unrelated(why)) => {
                ui.say(&format!("That is not the sign-in's return: {why}"));
            }
        }
    }
}

/// Why a callback address gave no code.
enum Callback {
    /// The identity provider refused the sign-in.
    Refused(String),
    /// The address is not this sign-in's return; wait for another.
    Unrelated(String),
}

/// The code in the callback address `got`.
fn code_in(got: &str, state: &CsrfToken, issuer: &str) -> Result<String, Callback> {
    let url = Url::parse(got.trim()).map_err(|e| Callback::Unrelated(e.to_string()))?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if url.path() != "/callback" {
        return Err(Callback::Unrelated(
            "it is not the /callback address".to_owned(),
        ));
    }
    if param("state").as_deref() != Some(state.secret()) {
        return Err(Callback::Unrelated(
            "its state is not this sign-in's".to_owned(),
        ));
    }
    if let Some(error) = param("error") {
        let detail = param("error_description").unwrap_or_default();
        return Err(Callback::Refused(format!(
            "sign-in refused: {error} {detail}"
        )));
    }
    if param("iss").is_some_and(|iss| iss.trim_end_matches('/') != issuer.trim_end_matches('/')) {
        return Err(Callback::Refused(
            "the sign-in came back from another issuer".to_owned(),
        ));
    }
    param("code").ok_or_else(|| Callback::Unrelated("it carries no code".to_owned()))
}

/// Serves the loopback redirect on a thread of its own until `done`, passing each
/// callback address on to `tx`.
fn listen(listener: TcpListener, redirect: &str, tx: &Sender<String>, done: &Arc<AtomicBool>) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    let base = redirect.trim_end_matches("/callback").to_owned();
    let (tx, done) = (tx.clone(), Arc::clone(done));
    let _ = std::thread::Builder::new()
        .name("moor-login".to_owned())
        .spawn(move || {
            while !done.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, &base, &tx),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
}

/// Reads the browser's request on `stream`, tells the person they can leave, and passes the
/// full address on.
fn answer(mut stream: TcpStream, base: &str, tx: &Sender<String>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    let _ = BufReader::new(Read::by_ref(&mut stream).take(8192)).read_line(&mut line);
    let target = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let page = "Signed in. You can close this tab.";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    if target.starts_with("/callback") {
        let _ = tx.send(format!("{base}{target}"));
    }
}
