//! The Postgres database a store opens, from a `postgres://` URL: its connection settings, how
//! TLS checks the server, and the URL with its password hidden for messages.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use postgres::config::SslMode as Negotiate;
use postgres::{Client, Config};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres_rustls::MakeRustlsConnect;
use url::Url;

/// The server option that makes every transaction on a connection read-only.
const READ_ONLY: &str = "-c default_transaction_read_only=on";

/// How long a connection attempt waits when the URL sets no `connect_timeout`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A Postgres database to connect to.
#[derive(Clone)]
pub(crate) struct Target {
    /// Connection settings, TLS negotiation included.
    config: Config,
    /// TLS that checks the server as the URL's `sslmode` says.
    tls: MakeRustlsConnect,
    /// The URL without its password or query, for messages.
    shown: String,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.shown)
    }
}

/// How TLS checks the server, as libpq's `sslmode` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SslMode {
    /// No TLS.
    Disable,
    /// TLS when the server offers it, without checking the server.
    Prefer,
    /// TLS, without checking the server.
    Require,
    /// TLS to a server whose certificate a trusted root signed.
    VerifyCa,
    /// As `VerifyCa`, and the certificate names the host.
    VerifyFull,
}

impl SslMode {
    /// The mode `word` names.
    fn parse(word: &str) -> Result<Self, String> {
        Ok(match word {
            "disable" => Self::Disable,
            "prefer" => Self::Prefer,
            "require" => Self::Require,
            "verify-ca" => Self::VerifyCa,
            "verify-full" => Self::VerifyFull,
            other => {
                return Err(format!(
                "sslmode `{other}` is not one of disable, prefer, require, verify-ca, verify-full"
            ))
            }
        })
    }

    /// Whether the connection asks for TLS, and whether it goes on without.
    const fn negotiate(self) -> Negotiate {
        match self {
            Self::Disable => Negotiate::Disable,
            Self::Prefer => Negotiate::Prefer,
            Self::Require | Self::VerifyCa | Self::VerifyFull => Negotiate::Require,
        }
    }
}

impl Target {
    /// The database `url` names. It is a `postgres://` or `postgresql://` URL whose query may set
    /// libpq's `sslmode` (default `prefer`) and `sslrootcert`, a PEM file of the roots that sign
    /// the server's certificate (default the system's roots). As in libpq, `require` with
    /// `sslrootcert` checks the certificate as `verify-ca` does.
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let parsed = Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
        if !matches!(parsed.scheme(), "postgres" | "postgresql") {
            return Err(format!(
                "the scheme is `{}`, not postgres or postgresql",
                parsed.scheme()
            ));
        }
        let shown = shown(&parsed);
        let (rest, mode, root) = split_tls(&parsed)?;
        let mode = match (mode, &root) {
            (SslMode::Require, Some(_)) => SslMode::VerifyCa,
            (mode, _) => mode,
        };
        let mut config: Config = rest.as_str().parse().map_err(|e| format!("{shown}: {e}"))?;
        config.ssl_mode(mode.negotiate());
        if config.get_connect_timeout().is_none() {
            config.connect_timeout(CONNECT_TIMEOUT);
        }
        let tls = tls(mode, root.as_deref()).map_err(|e| format!("{shown}: {e}"))?;
        Ok(Self { config, tls, shown })
    }

    /// This database for reads only: every transaction on its connections is read-only.
    pub(crate) fn read_only(&self) -> Self {
        let mut read_only = self.clone();
        let options = self.config.get_options().map_or_else(
            || READ_ONLY.to_owned(),
            |options| format!("{options} {READ_ONLY}"),
        );
        read_only.config.options(&options);
        read_only
    }

    /// A new connection.
    pub(super) fn connect(&self) -> Result<Client, postgres::Error> {
        self.config.connect(self.tls.clone())
    }

    /// The URL without its password or query.
    pub(crate) fn shown(&self) -> &str {
        &self.shown
    }

    /// The connection settings.
    #[cfg(test)]
    pub(crate) const fn config(&self) -> &Config {
        &self.config
    }
}

/// `url` without its password or query, which may hold a password too.
fn shown(url: &Url) -> String {
    let mut shown = url.clone();
    shown.set_query(None);
    shown.set_fragment(None);
    if shown.password().is_some() {
        let _ = shown.set_password(Some("***"));
    }
    shown.into()
}

/// `url` without `sslmode` and `sslrootcert`, which the Postgres client does not take as libpq
/// does, and their values. The other query fields stay as written.
fn split_tls(url: &Url) -> Result<(Url, SslMode, Option<String>), String> {
    let mut mode = SslMode::Prefer;
    let mut root = None;
    let mut kept = Vec::new();
    for field in url.query().unwrap_or_default().split('&') {
        match url::form_urlencoded::parse(field.as_bytes()).next() {
            Some((k, v)) if k == "sslmode" => mode = SslMode::parse(&v)?,
            Some((k, v)) if k == "sslrootcert" => root = Some(v.into_owned()),
            Some(_) => kept.push(field),
            None => {}
        }
    }
    let mut rest = url.clone();
    rest.set_query((!kept.is_empty()).then(|| kept.join("&")).as_deref());
    Ok((rest, mode, root))
}

/// TLS that checks the server as `mode` says, against the roots in the PEM file `root`, else the
/// system's.
fn tls(mode: SslMode, root: Option<&str>) -> Result<MakeRustlsConnect, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let check = match mode {
        SslMode::Disable | SslMode::Prefer | SslMode::Require => Check::Any,
        SslMode::VerifyCa | SslMode::VerifyFull => {
            let roots = Arc::new(roots(root)?);
            let webpki = WebPkiServerVerifier::builder_with_provider(roots, Arc::clone(&provider))
                .build()
                .map_err(|e| format!("cannot check the server's certificate: {e}"))?;
            if mode == SslMode::VerifyFull {
                Check::Named(webpki)
            } else {
                Check::Signed(webpki)
            }
        }
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("cannot set up TLS: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Verifier { check, provider }))
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

/// The roots in the PEM file `path`, else the system's.
fn roots(path: Option<&str>) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    let certs = match path {
        Some(path) => {
            let pem = std::fs::read(Path::new(path))
                .map_err(|e| format!("cannot read sslrootcert {path}: {e}"))?;
            pem_certs(&pem).map_err(|e| format!("sslrootcert {path}: {e}"))?
        }
        None => rustls_native_certs::load_native_certs().certs,
    };
    let (added, _) = roots.add_parsable_certificates(certs);
    if added == 0 {
        return Err(path.map_or_else(
            || "the system has no root certificates; set sslrootcert".to_owned(),
            |path| format!("sslrootcert {path} holds no certificate"),
        ));
    }
    Ok(roots)
}

/// The certificates in the PEM text `pem`.
fn pem_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject as _;
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

/// What the server's certificate must be.
#[derive(Debug)]
enum Check {
    /// Anything: the link is encrypted, and the server is not checked.
    Any,
    /// Signed by a trusted root.
    Signed(Arc<WebPkiServerVerifier>),
    /// Signed by a trusted root, and naming the host.
    Named(Arc<WebPkiServerVerifier>),
}

/// Checks the server's certificate as `sslmode` says, and its handshake signatures always.
#[derive(Debug)]
struct Verifier {
    /// What the certificate must be.
    check: Check,
    /// The signature algorithms handshakes are checked with.
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let (webpki, named) = match &self.check {
            Check::Any => return Ok(ServerCertVerified::assertion()),
            Check::Signed(webpki) => (webpki, false),
            Check::Named(webpki) => (webpki, true),
        };
        match webpki.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) if !named => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests;
