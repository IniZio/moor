use super::World;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct UserConfig {
    pub provider: Option<String>,
    pub max_boxes: Option<u32>,
    pub sleep_after_minutes: Option<u32>,
}

impl UserConfig {
    pub fn provider(mut self, p: &str) -> Self {
        self.provider = Some(p.to_string());
        self
    }

    pub fn max_boxes(mut self, n: u32) -> Self {
        self.max_boxes = Some(n);
        self
    }

    pub fn to_value(&self) -> Value {
        let mut root = Map::new();
        let mut crabbox = Map::new();
        if let Some(n) = self.max_boxes {
            crabbox.insert("max_boxes".into(), json!(n));
        }
        if let Some(n) = self.sleep_after_minutes {
            crabbox.insert("sleep_after_minutes".into(), json!(n));
        }
        if !crabbox.is_empty() {
            root.insert("drivers".into(), json!({ "crabbox": crabbox }));
        }
        if let Some(p) = &self.provider {
            root.insert("box".into(), json!({ "provider": p }));
        }
        Value::Object(root)
    }

    pub fn write(&self, w: &World) {
        write_value(&w.config_path(), &self.to_value());
    }
}

#[derive(Debug, Default, Clone)]
pub struct RepoConfig {
    pub model: Option<String>,
    pub setup: Option<String>,
}

impl RepoConfig {
    pub fn to_value(&self) -> Value {
        let mut root = Map::new();
        let mut agent = Map::new();
        if let Some(m) = &self.model {
            agent.insert("model".into(), json!(m));
        }
        if !agent.is_empty() {
            root.insert("agent".into(), Value::Object(agent));
        }
        if let Some(s) = &self.setup {
            root.insert("setup".into(), json!([s]));
        }
        Value::Object(root)
    }

    pub fn write(&self, repo_dir: &Path) {
        write_value(&repo_dir.join(".moor.yaml"), &self.to_value());
    }
}

fn write_value(path: &Path, v: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create config dir");
    }
    let text = serde_json::to_string_pretty(v).expect("serialize config");
    fs::write(path, text).expect("write config");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_empty_is_empty_object() {
        assert_eq!(UserConfig::default().to_value(), json!({}));
    }

    #[test]
    fn user_config_full_shape() {
        let c = UserConfig {
            sleep_after_minutes: Some(5),
            ..UserConfig::default().provider("static").max_boxes(2)
        };
        assert_eq!(
            c.to_value(),
            json!({
                "drivers": {"crabbox": {"max_boxes": 2, "sleep_after_minutes": 5}},
                "box": {"provider": "static"}
            })
        );
    }

    #[test]
    fn user_config_omits_unset_keys() {
        let v = UserConfig::default().max_boxes(1).to_value();
        assert_eq!(v, json!({"drivers": {"crabbox": {"max_boxes": 1}}}));
        let v = UserConfig::default().provider("aws").to_value();
        assert_eq!(v, json!({"box": {"provider": "aws"}}));
    }

    #[test]
    fn repo_config_shapes() {
        assert_eq!(RepoConfig::default().to_value(), json!({}));
        let c = RepoConfig {
            model: Some("haiku".into()),
            setup: Some("make setup".into()),
        };
        assert_eq!(
            c.to_value(),
            json!({"agent": {"model": "haiku"}, "setup": ["make setup"]})
        );
        let only_setup = RepoConfig {
            setup: Some("x".into()),
            ..RepoConfig::default()
        };
        assert_eq!(only_setup.to_value(), json!({"setup": ["x"]}));
    }
}
