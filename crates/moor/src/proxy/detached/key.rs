//! The ledger key the host mints for each supervisor it starts.

use std::io;

/// A fresh ledger key for one supervisor: 32 random bytes as base64url.
pub(super) fn mint_key() -> io::Result<zeroize::Zeroizing<String>> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(zeroize::Zeroizing::new(super::base64url(&bytes)))
}
