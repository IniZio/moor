//! `up --token`: a portal-minted token adds the machine with no browser step, and goes nowhere
//! but the pairing request.

use std::fmt::Write as _;

use serde_json::json;

use super::{assert_paired_as_hv, assert_service_serves_docker_boxes, last_line, world, World};

/// The controller keeps a grant of `token` for alice, as the portal's mint does.
fn grant(w: &World, token: &str) {
    let op = json!({"do": "grant", "ttl_secs": 600, "token": token,
        "owner": {"identity": "authgear:alice-sub", "email": "alice@example.com"}});
    w.pairings.lock().unwrap().step(&op);
}

impl World {
    fn up_with(&self, token: &str) -> super::Ran {
        self.agent(&[
            "up", "--server", &self.gate, "--name", "hv", "--token", token,
        ])
    }

    /// Every file the agent left under the home, as text, except the stand-ins' logs.
    fn kept_text(&self) -> String {
        let mut text = String::new();
        let mut dirs = vec![self.home.path().to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if let Ok(body) = std::fs::read_to_string(&path) {
                    writeln!(text, "{}: {body}", path.display()).unwrap();
                }
            }
        }
        text
    }
}

#[test]
fn up_with_a_token_adds_the_machine_without_a_browser_and_keeps_the_token_to_itself() {
    let w = world();
    grant(&w, "secret-token-1");
    let ran = w.up_with("secret-token-1");
    assert!(ran.ok, "{}", ran.said);
    assert!(
        ran.said.contains("added hv to your machines"),
        "{}",
        ran.said
    );
    assert!(!ran.said.contains("/pair?code="), "{}", ran.said);
    assert!(!ran.said.contains("secret-token-1"), "{}", ran.said);
    assert_eq!(last_line(&ran.said), "moor-machine-agent: linked: hv");
    assert_paired_as_hv(&w);
    assert_service_serves_docker_boxes(&w);
    assert!(
        !w.kept_text().contains("secret-token-1"),
        "the token was written to disk"
    );
}

#[test]
fn a_rerun_on_a_paired_machine_with_a_spent_token_changes_nothing() {
    let w = world();
    grant(&w, "secret-token-1");
    assert!(w.up_with("secret-token-1").ok);
    let again = w.up_with("secret-token-1");
    assert!(again.ok, "{}", again.said);
    assert!(again.said.contains("already paired"), "{}", again.said);
    assert_eq!(w.enrolled.lock().unwrap().len(), 1, "paired once");
}

#[test]
fn a_token_the_server_does_not_know_fails_plainly_and_pairs_nothing() {
    let w = world();
    let ran = w.up_with("nope");
    assert!(!ran.ok, "{}", ran.said);
    assert!(
        ran.said.contains("make a new one on the Machines page"),
        "{}",
        ran.said
    );
    assert!(!ran.said.contains("nope"), "{}", ran.said);
    assert!(!w
        .home
        .path()
        .join("state/moor-machine-agent/state.json")
        .exists());
    assert!(w.enrolled.lock().unwrap().is_empty());
}
