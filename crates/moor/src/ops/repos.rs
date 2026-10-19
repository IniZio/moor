//! `repos`: list the repos the controller knows.

use moor_core::request::ReposReq;
use moor_core::view::repos::{RepoView, ReposValue};

use crate::cx::Cx;

/// Known repos sorted by name.
pub(crate) fn run(cx: &Cx, _req: &ReposReq) -> ReposValue {
    let repos = cx
        .known_repos()
        .iter()
        .map(|(name, entry)| RepoView {
            name: name.clone(),
            url: entry.url.clone(),
        })
        .collect();
    ReposValue { repos }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use moor_core::config::{RepoEntry, RepoTable};
    use moor_core::Request;
    use serde_json::json;

    use super::*;
    use crate::ops::dispatch;
    use crate::testing::driver::FakeDriver;
    use crate::testing::proc::ScriptedProc;
    use crate::testing::surface::FakeSurface;

    fn cx(dir: &Path, repos: &[(&str, &str)]) -> Cx {
        let table: RepoTable = repos
            .iter()
            .map(|(name, url)| {
                let entry = RepoEntry {
                    url: (*url).to_string(),
                    default_branch: None,
                    moor_yaml: None,
                };
                ((*name).to_string(), entry)
            })
            .collect();
        Cx::fake(
            dir,
            Path::new("/home/me/proj"),
            Arc::new(FakeDriver::new()),
            Arc::new(FakeSurface::new(PathBuf::from("/s"))),
            Arc::new(ScriptedProc::new()),
        )
        .with_repos(table)
    }

    #[test]
    fn repos_come_back_sorted_by_name_with_url() {
        let dir = tempfile::tempdir().unwrap();
        let cx = cx(dir.path(), &[("zeta", "u/z"), ("alpha", "u/a")]);
        let got = run(&cx, &ReposReq {});
        let names: Vec<_> = got.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(got.repos.first().map(|r| r.url.as_str()), Some("u/a"));
    }

    #[test]
    fn no_repos_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let cx = cx(dir.path(), &[]);
        let got = dispatch(&cx, Request::Repos(ReposReq {})).unwrap();
        assert_eq!(got, json!({"repos": []}));
    }

    #[test]
    fn dispatch_returns_the_wire_shape() {
        let dir = tempfile::tempdir().unwrap();
        let cx = cx(dir.path(), &[("b", "u/b"), ("a", "u/a")]);
        let got = dispatch(&cx, Request::Repos(ReposReq {})).unwrap();
        let want = json!({"repos": [
            {"name": "a", "url": "u/a"},
            {"name": "b", "url": "u/b"},
        ]});
        assert_eq!(got, want);
    }
}
