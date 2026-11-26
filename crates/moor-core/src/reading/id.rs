//! The ids of a reading, its phrases and its effects. moor issues every one.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Text that is not an id of the kind asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a valid id")]
pub struct IdError(String);

/// A reading's id: a UUID in lower-case hex, sortable when it is a version 7 one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReadingId(String);

impl ReadingId {
    /// The id in `text`, 36 characters of lower-case hex with hyphens at 8, 13, 18 and 23.
    ///
    /// # Errors
    /// [`IdError`] for any other text.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        let ok = text.len() == 36
            && text.char_indices().all(|(i, c)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    c == '-'
                } else {
                    matches!(c, '0'..='9' | 'a'..='f')
                }
            });
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError(text.to_owned()))
        }
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReadingId {
    type Error = IdError;

    fn try_from(text: String) -> Result<Self, IdError> {
        Self::parse(&text)
    }
}

impl From<ReadingId> for String {
    fn from(id: ReadingId) -> Self {
        id.0
    }
}

impl fmt::Display for ReadingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An id moor numbers from 1 within one read, written as a letter and the number.
macro_rules! seq_id {
    ($name:ident, $letter:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(u32);

        impl $name {
            /// The id of the item at place `at`, from 0.
            pub(super) fn nth(at: usize) -> Self {
                Self(u32::try_from(at).unwrap_or(u32::MAX).saturating_add(1))
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(text: String) -> Result<Self, IdError> {
                text.strip_prefix($letter)
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|n| *n > 0 && text.len() == n.to_string().len() + 1)
                    .map(Self)
                    .ok_or(IdError(text))
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.to_string()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $letter, self.0)
            }
        }
    };
}

seq_id!(
    PhraseId,
    'p',
    "A phrase's id, `p1`, `p2`: stable within one read only."
);
seq_id!(
    EffectId,
    'e',
    "An effect's id, `e1`, `e2`: new in every read; only its key links reads."
);
