use std::path::{Path, PathBuf};

use parity::adapter::{EventKind, ExitClass, StartOpts, TaskState};
use parity::assert::assert_never_called;
use parity::fake::crabbox;
use parity::parity;
use parity::world::World;
use serde_json::json;

use super::*;
use crate::moor;

fn copied_events(w: &World, name: &str) -> usize {
    moor()
        .events(w, name)
        .iter()
        .filter(|e| e.kind == EventKind::Copied)
        .count()
}

parity!(
    cp_absolute_path_copies_without_sync,
    covers = ["B-20", "B-23", "REQ-24"],
    tags = [],
    {
        let f = fx();
        let src = seed_box_file(&f, "out.txt", "payload-1");
        let dest = host_dir(&f, "cp-abs").join("got.txt");
        let runs_before = user_runs(&f.w).len();
        let o = moor().cp(&f.w, &f.repo.path, &f.name, &src.to_string_lossy(), &dest);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "payload-1");
        let cps = verb_calls(&f.w, "cp");
        assert_eq!(cps.len(), 1);
        let c = &cps[0];
        assert_eq!(c.flag("--provider").as_deref(), Some("local-container"));
        assert_eq!(c.flag("--id").as_deref(), Some(f.slug.as_str()));
        assert!(c.argv.iter().any(|a| a.ends_with(&*src.to_string_lossy())));
        assert_eq!(c.argv.last().map(String::as_str), dest.to_str());
        assert_eq!(
            std::fs::canonicalize(&c.cwd).ok(),
            std::fs::canonicalize(&f.wt).ok()
        );
        assert_eq!(
            user_runs(&f.w).len(),
            runs_before,
            "cp never runs a command"
        );
        assert_eq!(copied_events(&f.w, &f.name), 1);
    }
);

parity!(
    cp_nonzero_exit_propagates_and_is_recorded,
    covers = ["B-20", "B-23"],
    tags = [],
    {
        let f = fx();
        crabbox::fail(&f.w, "cp");
        let dest = host_dir(&f, "cp-fail").join("got.txt");
        let o = moor().cp(&f.w, &f.repo.path, &f.name, "/nowhere", &dest);
        assert!(o.is_err());
        assert!(!dest.exists());
        assert_eq!(copied_events(&f.w, &f.name), 1);
    }
);

parity!(
    cp_relative_paths_resolve_against_workdir_and_cwd,
    covers = ["B-21", "B-22"],
    tags = [],
    {
        let f = fx();
        seed_box_file(&f, "rel.txt", "payload-2");
        let cwd = host_dir(&f, "cp-rel");
        let o = moor().cp(&f.w, &cwd, &f.name, "./rel.txt", Path::new("out.txt"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(
            std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
            "payload-2"
        );
        let probes: Vec<_> = verb_calls(&f.w, "run");
        let probe = probes
            .iter()
            .find(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
            .expect("workdir probe");
        assert!(has(probe, "--no-sync"));
        let cp = verb_calls(&f.w, "cp").remove(0);
        let want_src = f.w.box_root().join("work/rel.txt");
        assert!(cp
            .argv
            .iter()
            .any(|a| a.ends_with(&*want_src.to_string_lossy())));
        let host = PathBuf::from(cp.argv.last().unwrap());
        assert!(host.is_absolute());
        assert_eq!(
            std::fs::canonicalize(host.parent().unwrap()).unwrap(),
            std::fs::canonicalize(&cwd).unwrap()
        );
    }
);

parity!(
    cp_fails_when_workdir_cannot_be_resolved,
    covers = ["B-21"],
    tags = [],
    {
        let f = fx();
        seed_box_file(&f, "rel.txt", "x");
        crabbox::fail_match(&f.w, "run", "MOOR_WORKDIR", 1);
        let cwd = host_dir(&f, "cp-nowd");
        let o = moor().cp(&f.w, &cwd, &f.name, "rel.txt", Path::new("out.txt"));
        assert!(o.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
    }
);

parity!(
    cp_refused_without_a_ready_box,
    covers = ["B-26"],
    tags = [],
    {
        let f = fx();
        let m = moor();
        let dest = host_dir(&f, "cp-ref").join("o");
        let no_task = m.cp(&f.w, &f.repo.path, "nope", "/x", &dest);
        assert!(no_task.is_err());
        let blank = m.cp(&f.w, &f.repo.path, &f.name, "  ", &dest);
        assert!(blank.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
        crabbox::gone(&f.w, &f.slug);
        m.list(&f.w, &f.repo.path).ok();
        let gone = m.cp(&f.w, &f.repo.path, &f.name, "/x", &dest);
        assert!(gone.is_err());
        assert!(verb_calls(&f.w, "cp").is_empty());
        assert_never_called(&f.w.all_calls(), "crabbox", &["cp"]);

        let (w, repo) = prepare(None, json!({ "max_boxes": 1 }));
        start_task(&w, &repo, "cap");
        let second = w.task_name("wait");
        let o = moor().start(&w, &repo.path, &second, &StartOpts::default());
        assert_eq!(o.ok().state, TaskState::WaitingForBox);
        let r = moor().cp(&w, &repo.path, &second, "/x", &w.tmp_dir().join("o"));
        assert!(r.is_err());
        assert!(verb_calls(&w, "cp").is_empty());
    }
);
