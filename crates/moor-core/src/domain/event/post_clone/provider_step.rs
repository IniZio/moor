//! Timed steps a provider reports of a fork. moor stores them as it finds them and reads no step
//! by name.

use serde::{Deserialize, Serialize};

/// What marks a provider's line as one step.
const PREFIX: &str = "timing ";
/// The most steps one fork records, since the lines are another program's output.
pub const MAX_STEPS: usize = 64;
/// The longest step name kept.
const MAX_NAME: usize = 64;
/// The longest detail kept.
const MAX_DETAIL: usize = 200;

/// A step's name: lower-case words joined by `-` or `_`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepName(String);

impl StepName {
    /// The name in `text`, when it is one.
    fn parse(text: &str) -> Option<Self> {
        let word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
        (!text.is_empty() && text.len() <= MAX_NAME && text.chars().all(word))
            .then(|| Self(text.to_owned()))
    }

    /// The name's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One step of a fork.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStep {
    /// Which step.
    pub name: StepName,
    /// Time it took, in milliseconds.
    pub ms: u64,
    /// Whatever else the provider said of it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl ProviderStep {
    /// The step in `timing <name> ms=<n> [detail]`; none for any other line.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.strip_prefix(PREFIX)?.splitn(3, ' ');
        let name = StepName::parse(words.next()?)?;
        let ms = words.next()?.strip_prefix("ms=")?.parse().ok()?;
        let detail = words.next().unwrap_or_default();
        (detail.len() <= MAX_DETAIL && !detail.contains(char::is_control)).then(|| Self {
            name,
            ms,
            detail: detail.to_owned(),
        })
    }

    /// The steps among `lines`, dropping any other line, up to [`MAX_STEPS`].
    pub fn parse_all<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<Self> {
        lines
            .into_iter()
            .filter_map(Self::parse)
            .take(MAX_STEPS)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn a_step_line_parses_with_and_without_detail() {
        let step = ProviderStep::parse("timing cache-disk ms=41 group=g bytes=9").unwrap();
        assert_eq!(step.name.as_str(), "cache-disk");
        assert_eq!((step.ms, step.detail.as_str()), (41, "group=g bytes=9"));
        let step = ProviderStep::parse("timing fork-total ms=7").unwrap();
        assert_eq!((step.ms, step.detail.as_str()), (7, ""));
    }

    #[test]
    fn any_other_line_is_dropped() {
        let long_name = format!("timing {} ms=1", "n".repeat(MAX_NAME + 1));
        let long_detail = format!("timing n ms=1 {}", "d".repeat(MAX_DETAIL + 1));
        for line in [
            "",
            "passt ready",
            "timing",
            "timing n",
            "timing n 41",
            "timing n ms=",
            "timing n ms=-1",
            "timing n ms=1.5",
            "timing N ms=1",
            "timing a/b ms=1",
            "timing n ms=1 a\u{1b}[31m",
            "  timing n ms=1",
            &long_name,
            &long_detail,
        ] {
            assert_eq!(ProviderStep::parse(line), None, "{line:?}");
        }
        for line in [
            format!("timing {} ms=1", "n".repeat(MAX_NAME)),
            format!("timing n ms=1 {}", "d".repeat(MAX_DETAIL)),
        ] {
            assert!(ProviderStep::parse(&line).is_some(), "{line:?}");
        }
    }

    #[test]
    fn at_most_the_first_steps_are_kept() {
        let lines = vec!["timing n ms=1"; MAX_STEPS + 5];
        let noisy = std::iter::once("noise").chain(lines);
        assert_eq!(ProviderStep::parse_all(noisy).len(), MAX_STEPS);
    }

    #[test]
    fn an_event_without_detail_leaves_it_out_and_reads_back() {
        let step = ProviderStep::parse("timing n ms=1").unwrap();
        let text = serde_json::to_string(&step).unwrap();
        assert_eq!(text, r#"{"name":"n","ms":1}"#);
        assert_eq!(serde_json::from_str::<ProviderStep>(&text).unwrap(), step);
    }
}
