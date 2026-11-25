//! The request `start_box` sends, and how a start answers once its Box exists.

use moor_client::Client;
use moor_core::config::override_text;
use moor_core::gate::GatesDoc;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{AsReq, AskReq, Request as ControllerRequest, ShowReq, StartReq};
use moor_core::view::user::WhoamiValue;
use serde_json::Value;

use super::reply::Reply;
use super::{act_as, send, StartArgs};
use crate::Person;

/// The controller request that starts the Box `args` asks for.
pub(super) fn start_request(args: StartArgs) -> Result<ControllerRequest, String> {
    let mut repos = args
        .repos
        .iter()
        .map(|r| {
            ForgeRepo::parse(r).map_err(|_| format!("{r} is not a repo; give it as owner/name"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let repo = repos
        .next()
        .ok_or("name at least one repo, as owner/name")?;
    let others: Vec<ForgeRepo> = repos.collect();
    // One repo leaves the rest to the caller's `box.also`, as a start naming one always has.
    let also = (!others.is_empty()).then_some(others);
    let name = match args.name {
        Some(name) => name,
        None => format!("{}/mcp-{:08x}", repo.name.to_lowercase(), random()),
    };
    let name = BoxName::parse_allow_bare(&name).map_err(|e| {
        format!("{e}; pass name as <repo>/<label> in lowercase letters, digits and dashes")
    })?;
    let gates = match (args.gates, args.outcomes) {
        (None, Some(_)) => return Err("send outcomes with the gates they end".to_owned()),
        (None, None) => None,
        (Some(gates), outcomes) => Some(Box::new(GatesDoc {
            gates,
            outcomes: outcomes.unwrap_or_default(),
        })),
    };
    Ok(ControllerRequest::Start(StartReq {
        place: false,
        gates,
        placement: moor_core::decide::schedule::Placement::FirstOnly,
        cold: false,
        config: override_text(args.config),
        name,
        prompt: args.prompt,
        issue: None,
        base: args.base,
        model: None,
        node: args.node,
        no_agent: false,
        no_setup: args.no_setup.unwrap_or(false),
        resume: false,
        repo: Some(repo),
        parent: None,
        also,
        identity: None,
        task: None,
    }))
}

/// 32 random bits, for a Box name no one chose.
fn random() -> u32 {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let bits = RandomState::new().build_hasher().finish();
    u32::try_from(bits & u64::from(u32::MAX)).unwrap_or_default()
}

/// How often [`send_early`] looks at the Box.
const EARLY_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// How many polls [`send_early`] makes (about a minute) before giving up on the Box.
const EARLY_POLLS: u32 = 240;

/// Runs a `start` or a `retry` without waiting for it to finish: returns the Box's `show` as soon
/// as the Box exists, or its setup has restarted, while the controller's work carries on in the
/// background. Work that ends first, or a start for a Box that exists already, answers with its
/// own result. Other requests go through [`send`]. Either answer is shaped by `reply`.
pub(super) fn send_early(
    controller: &Client,
    person: &Person,
    request: ControllerRequest,
    reply: Reply,
) -> Result<serde_json::Value, String> {
    let (name, begun): (BoxName, fn(&Value) -> bool) = match &request {
        ControllerRequest::Start(start) => (start.name.clone(), |_| true),
        ControllerRequest::Retry(retry) => (retry.box_name.clone(), setup_restarted),
        _ => return send(controller, person, request, reply),
    };
    let show = ControllerRequest::Show(ShowReq {
        target: Some(name.into()),
        events: Some(5),
        agent: true,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
        everyone: false,
    });
    if matches!(request, ControllerRequest::Start(_))
        && act_as(controller, person, show.clone()).is_ok()
    {
        // The Box exists already: this start attaches or is refused as busy, and either answer
        // comes from the controller, so wait for it.
        return send(controller, person, request, reply);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (owned, who, done) = (controller.clone(), person.clone(), reply.clone());
    std::thread::spawn(move || {
        let _ = tx.send(send(&owned, &who, request, done));
    });
    for _ in 0..EARLY_POLLS {
        match rx.recv_timeout(EARLY_POLL) {
            Ok(done) => return done,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the start did not finish".to_owned());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        match send(controller, person, show.clone(), reply.clone()) {
            Ok(shown) if begun(&shown) => return Ok(shown),
            _ => {}
        }
    }
    Err("the Box did not appear in time; check box_status".to_owned())
}

/// Whether `shown` is a Box whose setup no longer stands failed: a retry took it up.
fn setup_restarted(shown: &Value) -> bool {
    shown.pointer("/box/lifecycle") != Some(&Value::from("setup_failed"))
}

/// Whether the controller lists `person` for agent places, so a send of theirs is a question.
/// A controller that does not say, or cannot be asked, means it is not.
pub(crate) fn asks(controller: &Client, person: &Person) -> bool {
    super::act_as(
        controller,
        person,
        ControllerRequest::Whoami(AsReq { identity: None }),
    )
    .ok()
    .and_then(|v| serde_json::from_value::<WhoamiValue>(v).ok())
    .is_some_and(|who| who.asks)
}

/// Asks `prompt` about `repos`, the first one the task's own, as `person`: the same request
/// `moor ask` sends. Answers the place that took it, with no work Box behind it. `origin` is
/// where a refusal sends the person for missing secrets.
pub(crate) fn ask_from_portal(
    controller: &Client,
    person: &Person,
    (repos, prompt): (Vec<String>, String),
    origin: std::sync::Arc<str>,
) -> Result<Value, String> {
    let repos = repos
        .iter()
        .map(|r| {
            ForgeRepo::parse(r).map_err(|_| format!("{r} is not a repo; give it as owner/name"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = ControllerRequest::Ask(AskReq { repos, prompt });
    send(controller, person, request, Reply::Asked(Some(origin)))
}

/// What a person asks of the portal to start.
pub(crate) struct NewBox {
    /// The repos, each as `owner/name`, first one first.
    pub(crate) repos: Vec<String>,
    /// What the agent should do first.
    pub(crate) prompt: String,
    /// The revision to branch from.
    pub(crate) base: Option<String>,
    /// The node to start on.
    pub(crate) node: Option<String>,
}

/// Starts a Box for `person` the way `start_box` does: the same request, answered once the Box
/// exists, while setup carries on. `origin` is where a refusal sends the person for missing secrets.
pub(crate) fn start_from_portal(
    controller: &Client,
    person: &Person,
    new: NewBox,
    origin: std::sync::Arc<str>,
) -> Result<Value, String> {
    let request = start_request(StartArgs {
        gates: None,
        outcomes: None,
        repos: new.repos,
        name: None,
        prompt: Some(new.prompt),
        base: new.base,
        node: new.node,
        config: None,
        no_setup: None,
        wait: None,
    })?;
    send_early(controller, person, request, Reply::Started(Some(origin)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(args: serde_json::Value) -> Result<ControllerRequest, String> {
        start_request(serde_json::from_value(args).expect("start_box arguments"))
    }

    #[test]
    fn start_box_starts_in_the_first_repo_and_clones_the_rest_beside_it() {
        assert!(matches!(
            start(serde_json::json!({"repos": ["o/a", "o/lib", "p/deploy"]})),
            Ok(ControllerRequest::Start(r))
                if r.repo == Some(ForgeRepo::new("o", "a"))
                    && r.also == Some(vec![ForgeRepo::new("o", "lib"), ForgeRepo::new("p", "deploy")])
        ));
        assert!(matches!(
            start(serde_json::json!({"repos": ["o/a"]})),
            Ok(ControllerRequest::Start(r)) if r.also.is_none()
        ));
        assert!(start(serde_json::json!({"repos": ["o/a", "lib"]})).is_err());
        assert!(start(serde_json::json!({"repos": []})).is_err());
    }
}
