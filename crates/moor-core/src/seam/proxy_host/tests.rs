use super::*;

fn spec(full: bool) -> ProxySpec {
    ProxySpec {
        box_id: BoxId::new(7),
        box_handle: full.then(|| BoxHandle::new("box-1")),
        box_repo: full.then(|| ForgeRepo {
            owner: "acme".into(),
            name: "widget".into(),
        }),
        rules: Vec::new(),
        user: crate::user::UserId::new(1),
        vault_key: "/config/vault.key".into(),
    }
}

#[test]
fn spec_round_trips_through_json() {
    for full in [true, false] {
        let s = spec(full);
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<ProxySpec>(&text).unwrap(), s);
    }
}

#[test]
fn spec_with_rule_round_trips_through_json() {
    use crate::config::proxy::{EndpointRule, ResolvedInject};
    let mut s = spec(true);
    s.rules = vec![ResolvedRule {
        name: "gh".into(),
        hosts: vec!["api.github.com".into()],
        credential: "forge".into(),
        inject: ResolvedInject::Header {
            header: "Authorization".into(),
            form: "Bearer {secret}".into(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".into(),
            path: "/repos/**".into(),
        }],
        box_env: [("GH_HOST".to_string(), "github.com".to_string())].into(),
    }];
    let text = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<ProxySpec>(&text).unwrap(), s);
}

#[test]
fn spec_json_holds_no_secret() {
    let text = serde_json::to_string(&spec(true)).unwrap().to_lowercase();
    assert!(!text.contains("secret"));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    for key in value.as_object().unwrap().keys() {
        assert!(!key.contains("secret"));
    }
}

#[test]
fn error_display_is_the_raw_message() {
    assert_eq!(ProxyHostError::Failed("boom".into()).to_string(), "boom");
    assert_eq!(
        ProxyHostError::Incomplete("half".into()).to_string(),
        "half"
    );
}
