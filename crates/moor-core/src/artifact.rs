//! Files published into a Box's artifact index: parsing, freshness and classification.

use std::collections::{BTreeMap, HashMap};

/// One file listed in a Box's artifact index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFile {
    /// Flat file name, as accepted by [`valid_name`].
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the content.
    pub sha256: String,
}

/// Broad class of an artifact, derived from its file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A still image.
    Screenshot,
    /// An animated GIF.
    Gif,
    /// A video recording.
    Video,
    /// A plain-text log.
    Log,
    /// Anything else.
    File,
}

impl ArtifactKind {
    /// Stable lowercase label for the kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Gif => "gif",
            Self::Video => "video",
            Self::Log => "log",
            Self::File => "file",
        }
    }

    /// Parse a label produced by [`ArtifactKind::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "screenshot" => Some(Self::Screenshot),
            "gif" => Some(Self::Gif),
            "video" => Some(Self::Video),
            "log" => Some(Self::Log),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

/// True for a flat name matching `[A-Za-z0-9][A-Za-z0-9._-]*`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// True for exactly 64 lowercase hex digits.
#[must_use]
pub fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parse `name<TAB>size<TAB>sha256` lines into entries sorted by name, plus the malformed-line count.
///
/// The last line wins for a repeated name. Blank lines are ignored and not counted.
#[must_use]
pub fn parse_index(text: &str) -> (Vec<ArtifactFile>, usize) {
    let mut bad = 0;
    let mut by_name: BTreeMap<String, ArtifactFile> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match parse_line(line) {
            Some(file) => {
                by_name.insert(file.name.clone(), file);
            }
            None => bad += 1,
        }
    }
    (by_name.into_values().collect(), bad)
}

/// Parse one index line, or `None` when it is malformed.
fn parse_line(line: &str) -> Option<ArtifactFile> {
    let mut parts = line.split('\t');
    let name = parts.next()?;
    let size = parts.next()?.trim().parse::<u64>().ok()?;
    let sha256 = parts.next()?.trim();
    if parts.next().is_some() || !valid_sha256(sha256) || !valid_name(name) {
        return None;
    }
    Some(ArtifactFile {
        name: name.to_owned(),
        size,
        sha256: sha256.to_owned(),
    })
}

/// Entries of `index` that are absent from `latest` or whose hash differs from the `latest` entry of that name.
#[must_use]
pub fn fresh(index: &[ArtifactFile], latest: &[ArtifactFile]) -> Vec<ArtifactFile> {
    let known: HashMap<&str, &str> = latest
        .iter()
        .map(|f| (f.name.as_str(), f.sha256.as_str()))
        .collect();
    index
        .iter()
        .filter(|f| known.get(f.name.as_str()) != Some(&f.sha256.as_str()))
        .cloned()
        .collect()
}

/// Lowercased text after the last `.`, or empty when there is none.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Classify `name` by its extension, case-insensitively.
#[must_use]
pub fn kind_of(name: &str) -> ArtifactKind {
    match extension(name).as_str() {
        "gif" => ArtifactKind::Gif,
        "mp4" | "mov" | "webm" => ArtifactKind::Video,
        "png" | "jpg" | "jpeg" | "webp" => ArtifactKind::Screenshot,
        "log" | "txt" => ArtifactKind::Log,
        _ => ArtifactKind::File,
    }
}

/// HTTP content type for `name`, by extension, case-insensitively.
#[must_use]
pub fn content_type(name: &str) -> &'static str {
    match extension(name).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "log" | "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "json" => "application/json",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests;
