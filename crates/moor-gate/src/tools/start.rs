//! The request `start_box` sends.

use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{Request as ControllerRequest, StartReq};

use super::StartArgs;

/// The controller request that starts the Box `args` asks for.
pub(super) fn start_request(args: StartArgs) -> Result<ControllerRequest, String> {
    let repo = ForgeRepo::parse(&args.repo)
        .map_err(|_| format!("{} is not a repo; give it as owner/name", args.repo))?;
    let also = args
        .also
        .map(|list| {
            list.iter()
                .map(|r| {
                    ForgeRepo::parse(r)
                        .map_err(|_| format!("{r} is not a repo; give it as owner/name"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let name = match args.name {
        Some(name) => name,
        None => format!("{}/mcp-{:08x}", repo.name.to_lowercase(), random()),
    };
    let name = BoxName::parse_allow_bare(&name).map_err(|e| {
        format!("{e}; pass name as <repo>/<label> in lowercase letters, digits and dashes")
    })?;
    Ok(ControllerRequest::Start(StartReq {
        name,
        prompt: args.prompt,
        issue: None,
        base: args.base,
        model: None,
        node: args.node,
        no_agent: false,
        cold: args.cold.unwrap_or(false),
        resume: false,
        repo: Some(repo),
        parent: None,
        also,
        identity: None,
    }))
}

/// 32 random bits, for a Box name no one chose.
fn random() -> u32 {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let bits = RandomState::new().build_hasher().finish();
    u32::try_from(bits & u64::from(u32::MAX)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(args: serde_json::Value) -> Result<ControllerRequest, String> {
        start_request(serde_json::from_value(args).expect("start_box arguments"))
    }

    #[test]
    fn start_box_asks_for_cold_only_when_told() {
        assert!(matches!(
            start(serde_json::json!({"repo": "o/a", "cold": true})),
            Ok(ControllerRequest::Start(StartReq { cold: true, .. }))
        ));
        assert!(matches!(
            start(serde_json::json!({"repo": "o/a"})),
            Ok(ControllerRequest::Start(StartReq { cold: false, .. }))
        ));
    }

    #[test]
    fn start_box_passes_the_other_repos_it_lists() {
        assert!(matches!(
            start(serde_json::json!({"repo": "o/a", "also": ["o/lib", "p/deploy"]})),
            Ok(ControllerRequest::Start(r))
                if r.also == Some(vec![ForgeRepo::new("o", "lib"), ForgeRepo::new("p", "deploy")])
        ));
        assert!(matches!(
            start(serde_json::json!({"repo": "o/a"})),
            Ok(ControllerRequest::Start(r)) if r.also.is_none()
        ));
        assert!(start(serde_json::json!({"repo": "o/a", "also": ["lib"]})).is_err());
    }
}
