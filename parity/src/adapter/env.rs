use serde_json::Value;

use super::types::opt_str;

#[derive(Debug, Clone, Default)]
pub struct EnvOpts {
    pub text: bool,
    pub set_env: Vec<(String, String)>,
    pub unset_env: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EnvItem {
    pub kind: String,
    pub name: String,
    pub source: String,
    pub detail: Option<String>,
    pub resolves: Option<bool>,
    pub error: Option<String>,
    pub delivery: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnvReport {
    pub repo: String,
    pub model: Option<String>,
    pub identity: String,
    pub items: Vec<EnvItem>,
}

impl EnvReport {
    pub fn from_json(v: &Value) -> EnvReport {
        let items = v
            .get("items")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(EnvItem::from_json).collect())
            .unwrap_or_default();
        let agent = v.get("agent").unwrap_or(&Value::Null);
        EnvReport {
            repo: opt_str(v, "repo").unwrap_or_default(),
            model: opt_str(agent, "model"),
            identity: v
                .get("github")
                .and_then(|g| opt_str(g, "identity"))
                .unwrap_or_default(),
            items,
        }
    }

    pub fn item(&self, kind: &str, name: &str) -> Option<&EnvItem> {
        self.items.iter().find(|i| i.kind == kind && i.name == name)
    }

    pub fn of_kind(&self, kind: &str) -> Vec<&EnvItem> {
        self.items.iter().filter(|i| i.kind == kind).collect()
    }

    pub fn credential(&self, name: &str) -> Option<&EnvItem> {
        self.item("credential", name)
    }

    pub fn provider(&self) -> Option<&str> {
        self.item("box", "provider")
            .and_then(|i| i.detail.as_deref())
    }

    pub fn size(&self) -> Option<&str> {
        self.item("box", "size").and_then(|i| i.detail.as_deref())
    }
}

impl EnvItem {
    fn from_json(v: &Value) -> EnvItem {
        EnvItem {
            kind: opt_str(v, "kind").unwrap_or_default(),
            name: opt_str(v, "name").unwrap_or_default(),
            source: opt_str(v, "source").unwrap_or_default(),
            detail: opt_str(v, "detail"),
            resolves: v.get("resolves").and_then(Value::as_bool),
            error: opt_str(v, "error"),
            delivery: opt_str(v, "delivery"),
        }
    }
}
