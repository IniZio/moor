use std::path::Path;

use parity::adapter::{ExitClass, TaskState};
use parity::fake::{herdr, Rule};
use parity::parity;

use super::support::*;

parity!(
    remove_refuses_open_unknown_and_bound_tasks,
    covers = ["A-98", "REQ-14"],
    tags = [],
    {
        let f = fx();
        let open = f.name("open");
        let t = f.start(&open);
        assert!(remove(&f, &open, true).class.is_err());
        assert!(Path::new(t.worktree.as_deref().unwrap()).is_dir());
        assert!(remove(&f, &f.name("nobody"), false).class.is_err());

        let wt = bound_worktree(&f, "feature/z");
        let bound = f.name("bound");
        f.start_with(&bound, &bind_opts(&wt));
        f.close(&bound);
        assert!(remove(&f, &bound, true).class.is_err());
        assert!(wt.is_dir());
        let listing = git_in(&f, &f.repo.path, &["branch", "--list", "feature/z"]);
        assert!(listing.contains("feature/z"));
    }
);

parity!(
    remove_refuses_unpushed_commits_until_discard,
    covers = ["A-99", "REQ-14"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "unp");
        commit_in(&f, &wt, "work.txt");
        assert!(remove(&f, &name, false).class.is_err());
        assert!(wt.is_dir() && branch_exists(&f, &name));
        let done = remove(&f, &name, true);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let view = done.ok();
        assert!(view.worktree_removed && view.branch_deleted && view.discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
    }
);

parity!(
    remove_refuses_uncommitted_files_but_ignores_moor_dirs,
    covers = ["A-99"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "dirty");
        std::fs::create_dir_all(wt.join(".moor")).unwrap();
        std::fs::write(wt.join(".moor/setup.log"), "x").unwrap();
        std::fs::write(wt.join("scratch.txt"), "x").unwrap();
        assert!(remove(&f, &name, false).class.is_err());
        assert!(wt.is_dir());
        std::fs::remove_file(wt.join("scratch.txt")).unwrap();
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        assert!(!done.ok().discarded);
        assert!(!wt.exists());
    }
);

parity!(
    remove_allows_commits_reachable_from_a_remote_ref,
    covers = ["A-99", "A-100"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "pushed");
        commit_in(&f, &wt, "work.txt");
        git_in(&f, &wt, &["push", "-q", "origin", &format!("moor/{name}")]);
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        assert!(!done.ok().discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
    }
);

parity!(
    remove_deletes_checkout_and_branch_and_repeat_is_a_noop,
    covers = ["A-100", "A-101"],
    tags = [],
    {
        let f = fx();
        let (name, wt) = closed_task(&f, "clean");
        let before = f.m.events(&f.w, &name);
        let stops = f.n("crabbox", "stop");
        let calls_before = f.w.calls("crabbox").len();
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let view = done.ok();
        assert!(view.worktree_removed && view.branch_deleted && !view.discarded);
        assert!(!wt.exists() && !branch_exists(&f, &name));
        assert_eq!(f.row(&name).state, TaskState::Closed);
        assert_eq!(f.n("crabbox", "stop"), stops);
        assert_eq!(f.w.calls("crabbox").len(), calls_before);
        assert!(f.m.events(&f.w, &name).len() > before.len());
        assert!(f.w.state_dir().join("registry.db").is_file());

        let again = remove(&f, &name, false);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert!(!again.ok().worktree_removed && !again.ok().branch_deleted);
    }
);

parity!(
    remove_uses_herdr_when_it_still_lists_the_worktree,
    covers = ["A-100"],
    tags = [],
    {
        let f = fx();
        let name = f.name("viaherdr");
        let t = f.start(&name);
        let wt = t.worktree.unwrap();
        f.close(&name);
        let branch = format!("moor/{name}");
        herdr::worktrees(&f.w, &[(&branch, &wt, Some("ws-open"))]);
        f.w.prepend_rules(
            "herdr",
            [Rule::on(&["worktree", "remove"]).exec(format!(
                "git -C '{}' worktree remove --force '{wt}' && printf '%s' '{{\"result\":{{}}}}'",
                f.repo.path.display()
            ))],
        );
        let done = remove(&f, &name, false);
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        let calls = herdr::calls_of(&f.w, "worktree", "remove");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].flag("--workspace").as_deref(), Some("ws-open"));
        assert!(calls[0].argv.iter().any(|a| a == "--force"));
        assert!(!Path::new(&wt).exists() && !branch_exists(&f, &name));
    }
);
