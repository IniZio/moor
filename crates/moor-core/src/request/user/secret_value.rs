//! A secret value carried in a request.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::secret::Secret;

/// A secret value in a request. Its JSON form is the text itself, because `secret-set` must
/// carry it over the socket; `Debug` never shows it. Serialize a request only to send it.
pub struct SecretValue(Secret);

impl SecretValue {
    /// Wraps the value's bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Secret::new(bytes))
    }

    /// The value.
    #[must_use]
    pub const fn secret(&self) -> &Secret {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self::new(self.0.expose().to_vec())
    }
}

impl PartialEq for SecretValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose() == other.0.expose()
    }
}

impl Eq for SecretValue {}

impl Serialize for SecretValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = std::str::from_utf8(self.0.expose())
            .map_err(|_| serde::ser::Error::custom("a secret value must be UTF-8 text"))?;
        s.serialize_str(text)
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|text| Self::new(text.into_bytes()))
    }
}
