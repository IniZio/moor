use std::path::PathBuf;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ProxyCheck {
    pub task: String,
    pub supervisor_alive: bool,
    pub tunnel: String,
    pub box_port: Option<u16>,
    pub host_port: Option<u16>,
    pub identity: Option<String>,
    pub box_status: Option<u16>,
    pub box_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProxyEndpoint {
    pub port: u16,
    pub box_port: u16,
    pub nonce: String,
    pub nonce_path: PathBuf,
}

fn port(v: &Value, key: &str) -> Option<u16> {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

impl ProxyCheck {
    pub fn from_json(v: &Value) -> ProxyCheck {
        ProxyCheck {
            task: text(v, "task").unwrap_or_default(),
            supervisor_alive: v
                .get("supervisor_alive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            tunnel: text(v, "tunnel").unwrap_or_default(),
            box_port: port(v, "box_port"),
            host_port: port(v, "host_port"),
            identity: text(v, "identity"),
            box_status: port(v, "box_status"),
            box_error: text(v, "box_error"),
        }
    }
}

impl ProxyEndpoint {
    pub fn from_state(path: PathBuf, v: &Value) -> Option<ProxyEndpoint> {
        Some(ProxyEndpoint {
            port: port(v, "host_port").filter(|p| *p > 0)?,
            box_port: port(v, "box_port")?,
            nonce: text(v, "nonce")?,
            nonce_path: path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn check_parses_report() {
        let c = ProxyCheck::from_json(&json!({
            "task": "t", "supervisor_alive": true, "tunnel": "up",
            "box_port": 20001, "host_port": null, "box_status": 403
        }));
        assert!(c.supervisor_alive);
        assert_eq!(
            (c.box_port, c.host_port, c.box_status),
            (Some(20001), None, Some(403))
        );
    }

    #[test]
    fn endpoint_needs_a_host_port() {
        let p = PathBuf::from("/s/proxy/t.json");
        let starting = json!({"nonce": "n", "box_port": 20001, "host_port": 0});
        assert!(ProxyEndpoint::from_state(p.clone(), &starting).is_none());
        let ready = json!({"nonce": "n", "box_port": 20001, "host_port": 4000});
        assert_eq!(ProxyEndpoint::from_state(p, &ready).unwrap().port, 4000);
    }
}
