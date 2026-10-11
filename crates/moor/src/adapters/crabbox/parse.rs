//! Pure parsers for crabbox output.

use serde::Deserialize;
use serde_json::Value;

/// Facts read from `crabbox warmup` stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Warmup {
    /// Lease id on the `leased` line.
    pub(super) id: String,
    /// Work root on the `ready` line, when printed.
    pub(super) workroot: Option<String>,
}

/// Reads the `leased` and `ready` lines; `None` when there is no `leased` line.
pub(super) fn parse_warmup(text: &str) -> Option<Warmup> {
    let id = text
        .lines()
        .find_map(|l| l.strip_prefix("leased "))
        .and_then(|rest| rest.split_whitespace().next())?
        .to_owned();
    let workroot = text
        .lines()
        .filter(|l| l.starts_with("ready "))
        .find_map(|l| l.split_once("workroot=").map(|(_, w)| w.trim().to_owned()))
        .filter(|w| !w.is_empty());
    Some(Warmup { id, workroot })
}

/// One Box from `crabbox list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListItem {
    /// Provider, when the item names one.
    pub(super) provider: Option<String>,
    /// Name `stop --id` accepts.
    pub(super) name: String,
    /// Whether the Box sleeps.
    pub(super) asleep: bool,
}

/// Raw list entry; unknown fields are ignored.
#[derive(Deserialize)]
struct RawItem {
    /// Numeric or string id.
    id: Option<Value>,
    /// Server name.
    name: Option<String>,
    /// Short name, when top-level.
    slug: Option<String>,
    /// Server status.
    status: Option<String>,
    /// Provider, when listed.
    provider: Option<String>,
    /// Crabbox labels.
    labels: Option<Labels>,
}

/// The label subset moor reads.
#[derive(Deserialize)]
struct Labels {
    /// Short name.
    slug: Option<String>,
    /// Provider label.
    provider: Option<String>,
    /// Lease label.
    lease: Option<String>,
}

/// Status values that mean the Box is not running.
const ASLEEP: &[&str] = &[
    "off",
    "stopped",
    "paused",
    "sleeping",
    "suspended",
    "hibernated",
];

/// Parses `list --json`; entries without any usable name are skipped.
///
/// # Errors
/// Returns the JSON error when the text is not an array of objects.
pub(super) fn parse_list(json: &str) -> Result<Vec<ListItem>, serde_json::Error> {
    let raw: Vec<RawItem> = serde_json::from_str(json)?;
    Ok(raw.into_iter().filter_map(item).collect())
}

/// Turns one raw entry into a listing, preferring slug over name over id.
fn item(r: RawItem) -> Option<ListItem> {
    let (label_slug, label_provider, lease) = r
        .labels
        .map_or((None, None, None), |l| (l.slug, l.provider, l.lease));
    let id = r.id.map(|v| match v {
        Value::String(s) => s,
        other => other.to_string(),
    });
    let name = [r.slug, label_slug, r.name, id, lease]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())?;
    let asleep = r
        .status
        .is_some_and(|s| ASLEEP.contains(&s.to_ascii_lowercase().as_str()));
    Some(ListItem {
        provider: r.provider.or(label_provider).filter(|p| !p.is_empty()),
        name,
        asleep,
    })
}

/// Parses `inspect --json` and returns its `workroot`, when set.
///
/// # Errors
/// Returns the JSON error when the text is not a JSON object.
pub(super) fn parse_inspect_workroot(json: &str) -> Result<Option<String>, serde_json::Error> {
    #[derive(Deserialize)]
    struct View {
        /// Remote work root.
        workroot: Option<String>,
    }
    let v: View = serde_json::from_str(json)?;
    Ok(v.workroot.filter(|w| w.starts_with('/')))
}
