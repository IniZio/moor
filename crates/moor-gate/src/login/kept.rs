//! A cookie key kept in a file, for a gate run without `--cookie-key`: the first start makes
//! it, later starts read it, so a restart or an upgrade ends no session.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::rand_core::RngCore as _;
use chacha20poly1305::aead::OsRng;

/// Random bytes in a made key: 64 characters once encoded.
const KEY_BYTES: usize = 48;

/// The key in the file at `path`, made with mode `0600` when there is none. The key is written
/// beside it and linked in whole, so two gates making it at once agree on the first one's.
pub(super) fn read_or_make(path: &Path) -> Result<String, String> {
    let shown = path.display();
    match fs::read_to_string(path) {
        Ok(key) => return Ok(key.trim().to_owned()),
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("{shown}: {e}")),
        Err(_) => {}
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut bytes = [0_u8; KEY_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let key = URL_SAFE_NO_PAD.encode(bytes);
    let draft = path.with_extension(format!("{}.new", std::process::id()));
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&draft)
        .and_then(|mut file| file.write_all(key.as_bytes()));
    let linked = written.and_then(|()| fs::hard_link(&draft, path));
    let _ = fs::remove_file(&draft);
    match linked {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => read_or_make(path),
        Err(e) => Err(format!("{shown}: {e}")),
    }
}
