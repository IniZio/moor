//! Something a Box's agent made outside moor: a pull request comment, a commit status, a Figma
//! comment, an upload. moor stores and shows it; no code branches on its kind.

use serde::{Deserialize, Serialize};

/// The longest title, in bytes.
pub const TITLE_MAX: usize = 200;

/// The longest URL, in bytes.
pub const URL_MAX: usize = 2048;

/// The most outputs one Box may report.
pub const PER_BOX_MAX: usize = 100;

/// What every first message tells the agent about outputs.
pub const REPORT_OUTPUT_BRIEF: &str = "After each write outside moor, such as a pull request \
                                       comment, call report_output with its kind, URL and title.";

/// The longest kind, in bytes.
const KIND_MAX: usize = 64;

/// What an output is, in 1 to 3 dot-separated words of `[a-z0-9-]`, such as `github.comment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OutputKind(String);

impl OutputKind {
    /// `text` as a kind.
    ///
    /// # Errors
    /// Why `text` is not 1 to 3 dot-separated words of `[a-z0-9-]`, 64 bytes at most.
    pub fn parse(text: &str) -> Result<Self, String> {
        let words: Vec<&str> = text.split('.').collect();
        let word_ok = |w: &&str| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if text.len() > KIND_MAX || words.len() > 3 || !words.iter().all(word_ok) {
            return Err(format!(
                "kind {text:?} must be 1 to 3 dot-separated words of a-z, 0-9 and -, such as github.comment"
            ));
        }
        Ok(Self(text.to_owned()))
    }

    /// The kind as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OutputKind {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl From<OutputKind> for String {
    fn from(kind: OutputKind) -> Self {
        kind.0
    }
}

/// One output as the agent reported it, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// What it is.
    kind: OutputKind,
    /// Where it lives, an `https` address.
    url: String,
    /// One trimmed line.
    title: String,
}

impl Output {
    /// The output of `kind` at `url`, titled `title`.
    ///
    /// # Errors
    /// Why one of them is refused: a bad kind, a URL that is not `https` or too long, or a
    /// title that is empty, not one line, or too long.
    pub fn parse(kind: &str, url: &str, title: &str) -> Result<Self, String> {
        let kind = OutputKind::parse(kind)?;
        let host = url.strip_prefix("https://").unwrap_or_default();
        if host.is_empty()
            || host.starts_with('/')
            || url.len() > URL_MAX
            || url.contains(char::is_whitespace)
        {
            return Err(format!(
                "url must be an https address of at most {URL_MAX} bytes"
            ));
        }
        let title = title.trim();
        if title.is_empty() || title.len() > TITLE_MAX || title.contains(['\n', '\r']) {
            return Err(format!("title must be one line of 1 to {TITLE_MAX} bytes"));
        }
        Ok(Self {
            kind,
            url: url.to_owned(),
            title: title.to_owned(),
        })
    }

    /// What it is.
    #[must_use]
    pub const fn kind(&self) -> &OutputKind {
        &self.kind
    }

    /// Where it lives.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Its one-line title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
}

#[cfg(test)]
mod tests;
