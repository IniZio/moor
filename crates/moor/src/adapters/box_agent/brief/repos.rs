//! The brief's list of the repos a Box holds, when it holds more than one.

use std::fmt::Write;

use moor_core::seam::engine::HeldRepo;

/// Lists every repo of `repos` with its folder, and how to commit, push and open a pull request in
/// each; nothing for a Box with one repo or none.
pub(super) fn section(repos: &[HeldRepo], branch: &str, out: &mut String) {
    if repos.len() < 2 {
        return;
    }
    _ = write!(
        out,
        "## This Box's repos\n\nThis Box holds {} repos, each a git checkout on branch `{branch}`:\n\n",
        repos.len()
    );
    for (i, t) in repos.iter().enumerate() {
        let own = if i == 0 {
            ", your working directory"
        } else {
            ""
        };
        _ = writeln!(out, "- `{}` at `{}`{own}", t.repo, t.dir);
    }
    out.push_str(
        "\nWork in each repo the job needs. Each of these folders counts as a working \
         directory: write and commit there, and push from there with the push command above. \
         Open a separate pull request for each repo you change, and end your final message \
         with every pull request URL. Reports and published files stay in your working \
         directory's `.moor/`.\n\n",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(repo: &str, dir: &str) -> HeldRepo {
        HeldRepo {
            repo: repo.to_owned(),
            dir: dir.to_owned(),
        }
    }

    #[test]
    fn one_repo_or_none_adds_nothing() {
        let mut out = String::new();
        section(&[], "moor/t1", &mut out);
        section(&[tree("o/app", "/w/app")], "moor/t1", &mut out);
        assert_eq!(out, "");
    }

    #[test]
    fn names_every_repo_its_folder_and_a_pull_request_each() {
        let mut out = String::new();
        let repos = [tree("o/app", "/w/app"), tree("o/lib", "/w/lib")];
        section(&repos, "moor/t1", &mut out);
        assert!(out.contains("This Box holds 2 repos, each a git checkout on branch `moor/t1`:\n\n- `o/app` at `/w/app`, your working directory\n- `o/lib` at `/w/lib`\n"), "{out}");
        assert!(
            out.contains("Each of these folders counts as a working directory"),
            "{out}"
        );
        assert!(
            out.contains("Open a separate pull request for each repo you change"),
            "{out}"
        );
    }
}
