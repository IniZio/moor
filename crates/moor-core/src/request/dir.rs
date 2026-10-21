//! The caller's directory: a CLI names it for a request it sends with no repo, because the
//! controller never reads a directory of its own.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Request;

/// Arguments of `in`: another request, run on the caller's checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InReq {
    /// The caller's working directory, absolute.
    pub dir: String,
    /// The request to run there.
    pub request: Box<Request>,
}

impl Request {
    /// Splits off the directory an `in` request names; any other request names none.
    ///
    /// # Errors
    /// The refusal for a relative directory or an `in` around another `in`.
    pub fn split_dir(self) -> Result<(Option<PathBuf>, Self), String> {
        let Self::In(at) = self else {
            return Ok((None, self));
        };
        let dir = PathBuf::from(at.dir);
        if !dir.is_absolute() {
            return Err(format!("in names a relative directory {}", dir.display()));
        }
        match *at.request {
            Self::In(_) => Err("an in request cannot wrap another in request".to_owned()),
            inner => Ok((Some(dir), inner)),
        }
    }
}

#[cfg(test)]
mod tests;
