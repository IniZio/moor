//! Tests for the persisted per-Box CA.

use std::os::unix::fs::PermissionsExt;

use moor_core::BoxId;
use nono_proxy::tls_intercept::EphemeralCa;

use super::*;

/// Makes and installs a CA for the Box the way the controller does.
fn load_or_create(state: &Path, box_id: BoxId) -> std::io::Result<BoxCa> {
    let (key, cert) = generate(box_id)?;
    install(state, box_id, (key.as_bytes(), &cert))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn files_and_dir_are_owner_only() {
    let state = tempfile::tempdir().unwrap();
    load_or_create(state.path(), BoxId::new(7)).unwrap();
    let dir = ca_dir(state.path(), BoxId::new(7));
    assert_eq!(dir, state.path().join("proxy").join("7"));
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("ca-key.pem")), 0o600);
    assert_eq!(mode(&dir.join("ca.pem")), 0o600);
}

#[test]
fn loose_modes_are_tightened() {
    let state = tempfile::tempdir().unwrap();
    load_or_create(state.path(), BoxId::new(7)).unwrap();
    let dir = ca_dir(state.path(), BoxId::new(7));
    for (p, m) in [
        (&dir, 0o755),
        (&dir.join("ca-key.pem"), 0o644),
        (&dir.join("ca.pem"), 0o644),
    ] {
        std::fs::set_permissions(p, Permissions::from_mode(m)).unwrap();
    }
    load_or_create(state.path(), BoxId::new(7)).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("ca-key.pem")), 0o600);
    assert_eq!(mode(&dir.join("ca.pem")), 0o600);
}

#[test]
fn remove_deletes_and_is_idempotent() {
    let state = tempfile::tempdir().unwrap();
    load_or_create(state.path(), BoxId::new(7)).unwrap();
    remove(state.path(), BoxId::new(7)).unwrap();
    assert!(!ca_dir(state.path(), BoxId::new(7)).exists());
    remove(state.path(), BoxId::new(7)).unwrap();
}

#[test]
fn a_key_that_does_not_match_the_cert_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let (_, cert) = generate(BoxId::new(7)).unwrap();
    let other = EphemeralCa::generate().unwrap();
    let err = install(
        state.path(),
        BoxId::new(7),
        (other.key_pem().as_bytes(), &cert),
    )
    .unwrap_err();
    assert!(err.to_string().contains("ca-key.pem"), "{err}");
}

#[test]
fn debug_shows_no_key_or_cert() {
    let state = tempfile::tempdir().unwrap();
    let ca = load_or_create(state.path(), BoxId::new(7)).unwrap();
    let text = format!("{ca:?}");
    assert!(!text.contains("PRIVATE KEY"));
    assert!(!text.contains("CERTIFICATE"));
    assert!(!text.contains(ca.cert_pem.lines().nth(1).unwrap()));
}

#[test]
fn preloaded_round_trips_and_names_the_box() {
    let state = tempfile::tempdir().unwrap();
    let ca = load_or_create(state.path(), BoxId::new(7)).unwrap();
    let pre = ca.preloaded();
    assert_eq!(pre.cert_pem, ca.cert_pem);
    let rebuilt = EphemeralCa::from_existing(&pre.key_der, &pre.cert_pem).unwrap();
    assert_eq!(rebuilt.cert_pem(), ca.cert_pem);
    let cn = b"moor box 7";
    let der = rcgen_free_der(&ca.cert_pem);
    assert!(der.windows(cn.len()).any(|w| w == cn));
}

/// Decodes the first PEM block's base64 body without extra dependencies.
fn rcgen_free_der(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in body.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | u32::try_from(table.iter().position(|t| *t == c).unwrap()).unwrap();
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).unwrap());
        }
    }
    out
}
