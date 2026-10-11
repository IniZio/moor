use std::path::PathBuf;

use serde_json::Value;

use super::{opt_str, TaskView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub kind: String,
    pub version: i64,
    pub backend: String,
    pub link: String,
    pub title: Option<String>,
}

impl ReportRow {
    pub fn from_json(v: &Value) -> ReportRow {
        ReportRow {
            kind: opt_str(v, "kind").unwrap_or_default(),
            version: v.get("version").and_then(Value::as_i64).unwrap_or(0),
            backend: opt_str(v, "backend").unwrap_or_default(),
            link: opt_str(v, "link").unwrap_or_default(),
            title: opt_str(v, "title"),
        }
    }

    /// Host path of the stored copy, when the link is a `file://` URL.
    pub fn copy_path(&self) -> Option<PathBuf> {
        self.link.strip_prefix("file://").map(PathBuf::from)
    }
}

/// A request to present a Report. Invalid combinations are expressible on
/// purpose: the CLI must reject them as usage errors.
#[derive(Debug, Clone, Default)]
pub struct ReportReq {
    pub kind: String,
    pub file: Option<PathBuf>,
    pub link: Option<String>,
    pub version: Option<i64>,
    pub title: Option<String>,
}

impl ReportReq {
    pub fn file(kind: &str, path: impl Into<PathBuf>) -> ReportReq {
        ReportReq {
            kind: kind.to_string(),
            file: Some(path.into()),
            ..Default::default()
        }
    }

    pub fn link(kind: &str, url: &str) -> ReportReq {
        ReportReq {
            kind: kind.to_string(),
            link: Some(url.to_string()),
            ..Default::default()
        }
    }

    pub fn version(mut self, n: i64) -> ReportReq {
        self.version = Some(n);
        self
    }

    pub fn title(mut self, t: &str) -> ReportReq {
        self.title = Some(t.to_string());
        self
    }
}

impl TaskView {
    /// Reports carried on a `list` or `pane` row.
    pub fn reports(&self) -> Vec<ReportRow> {
        self.raw
            .get("reports")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(ReportRow::from_json).collect())
            .unwrap_or_default()
    }
}
