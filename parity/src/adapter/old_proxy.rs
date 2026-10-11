use std::path::Path;

use serde_json::Value;

use super::old::OldCli;
use super::proxy::{ProxyCheck, ProxyEndpoint};
use super::Outcome;
use crate::world::World;

impl OldCli {
    pub(super) fn proxy_check_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
    ) -> Outcome<ProxyCheck> {
        let args = ["proxy", "check", name, "--json"].map(String::from);
        self.run(w, cwd, &args, ProxyCheck::from_json)
    }

    pub(super) fn proxy_endpoint_impl(&self, w: &World, name: &str) -> Option<ProxyEndpoint> {
        let path = w.state_dir().join("proxy").join(format!("{name}.json"));
        let doc: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        ProxyEndpoint::from_state(path, &doc)
    }
}
