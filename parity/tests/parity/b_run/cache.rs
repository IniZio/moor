use std::path::PathBuf;

use parity::adapter::ExitClass;
use parity::fake::crabbox;
use parity::parity;
use parity::world::World;
use serde_json::json;

use super::*;
use crate::moor;

fn probe(f: &Fx, dirs: &[&str]) -> String {
    let mut cmd = String::new();
    for d in dirs {
        cmd.push_str(&format!(
            "if [ -e '{d}/dep.txt' ]; then echo \"KEPT {d}\"; else echo \"GONE {d}\"; fi; "
        ));
    }
    for d in dirs {
        cmd.push_str(&format!("mkdir -p '{d}'; echo x > '{d}/dep.txt'; "));
    }
    let o = rec(f, "unit", &cmd);
    assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
    o.stdout
}

fn kept(out: &str, d: &str) -> bool {
    assert!(
        out.contains(&format!("KEPT {d}")) || out.contains(&format!("GONE {d}")),
        "{d} not probed in {out}"
    );
    out.contains(&format!("KEPT {d}"))
}

const LOCKS: &[(&str, &str)] = &[
    ("package-lock.json", "node_modules"),
    ("uv.lock", ".venv"),
    ("Gemfile.lock", "vendor/bundle"),
    ("composer.lock", "vendor/composer-deps"),
    ("Podfile.lock", "Pods"),
];

parity!(
    record_keeps_dependency_dirs_by_lockfile,
    covers = ["B-43", "B-44", "B-45", "REQ-27"],
    tags = [],
    {
        let f = fx();
        let mut files: Vec<(&str, &str)> = LOCKS.iter().map(|(l, _)| (*l, "v1")).collect();
        files.push((
            ".gitignore",
            "node_modules/\n.venv/\nvendor/\nPods/\n.next/\ndist/\n",
        ));
        wt_commit(&f, &files);
        let dirs: Vec<&str> = LOCKS.iter().map(|(_, d)| *d).collect();
        let first = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&first, d)), "{first}");
        let second = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| kept(&second, d)), "{second}");

        wt_commit(&f, &[("uv.lock", "v2")]);
        let after_edit = probe(&f, &dirs);
        assert!(!kept(&after_edit, ".venv"), "{after_edit}");
        assert!(kept(&after_edit, "node_modules"), "{after_edit}");

        wt_commit(&f, &[(".nvmrc", "20")]);
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
        wt_commit(&f, &[(".nvmrc", "22")]);
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_cache_key_includes_setup_text,
    covers = ["B-42", "B-44"],
    tags = [],
    {
        let f = fx_with(Some(json!({ "setup": ["echo one >> .moor/setup-runs"] })));
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let runs = |f: &Fx| {
            std::fs::read_to_string(f.wt.join(".moor/setup-runs"))
                .unwrap_or_default()
                .lines()
                .count()
        };
        let at_start = runs(&f);
        assert_eq!(at_start, 1, "setup ran once at start");
        probe(&f, &["node_modules"]);
        assert_eq!(runs(&f), 2, "setup is re-run after the clean");
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
        let cfg = json!({ "setup": ["echo one >> .moor/setup-runs", "true"] });
        std::fs::write(f.repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        assert!(!kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_failing_setup_step_stops_the_command,
    covers = ["B-42"],
    tags = [],
    {
        let cfg = json!({ "setup": ["test ! -e .moor/break-setup"] });
        let f = fx_with(Some(cfg));
        wt_commit(&f, &[("a.txt", "a")]);
        put(&f.wt, ".moor/break-setup", "1");
        let marker = f.w.box_root().join("command-ran");
        let o = rec(&f, "unit", &format!("touch '{}'", marker.display()));
        assert!(o.is_err());
        assert!(!marker.exists(), "command must not run after failed prep");
        let rows = moor().runs(&f.w, &f.name);
        assert_ne!(rows[0].exit, 0);
    }
);

parity!(
    record_tool_caches_follow_the_node_lockfile,
    covers = ["B-46"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (
                    ".gitignore",
                    "node_modules/\n.next/\n.turbo/\n.nx/\ndist/\n",
                ),
            ],
        );
        let dirs = [
            "node_modules",
            ".next/cache",
            ".next/server",
            ".turbo",
            ".nx/cache",
            "dist",
        ];
        probe(&f, &dirs);
        let hit = probe(&f, &dirs);
        for d in ["node_modules", ".next/cache", ".turbo", ".nx/cache"] {
            assert!(kept(&hit, d), "{d} kept on a key match: {hit}");
        }
        for d in [".next/server", "dist"] {
            assert!(!kept(&hit, d), "{d} is a build output: {hit}");
        }
        wt_commit(&f, &[("package-lock.json", "v2")]);
        let miss = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&miss, d)), "{miss}");
    }
);

parity!(
    record_failed_run_does_not_vouch_for_the_cache,
    covers = ["B-45", "REQ-27"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let bad = rec(
            &f,
            "unit",
            "mkdir -p node_modules && echo x > node_modules/dep.txt; exit 3",
        );
        assert_eq!(bad.exit, 3);
        let after = probe(&f, &["node_modules"]);
        assert!(!kept(&after, "node_modules"), "{after}");
        assert!(kept(&probe(&f, &["node_modules"]), "node_modules"));
    }
);

parity!(
    record_cache_ignores_vendored_unsafe_and_uncached_ecosystems,
    covers = ["B-43", "B-48"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                (
                    ".gitignore",
                    "node_modules/\n.venv/\ntarget/\n.dart_tool/\n",
                ),
                ("vendor/foo/package-lock.json", "v1"),
                ("my dir/package-lock.json", "v1"),
                ("requirements.txt", "x"),
                ("Cargo.lock", "x"),
                ("go.sum", "x"),
                ("pubspec.lock", "x"),
            ],
        );
        let dirs = [
            "vendor/foo/node_modules",
            "my dir/node_modules",
            ".venv",
            "target",
            ".dart_tool",
        ];
        probe(&f, &dirs);
        let again = probe(&f, &dirs);
        assert!(dirs.iter().all(|d| !kept(&again, d)), "{again}");
    }
);

parity!(
    record_with_too_many_lockfiles_keeps_nothing,
    covers = ["B-47"],
    tags = [],
    {
        let f = fx();
        let mut files: Vec<(String, &str)> = (0..64)
            .map(|i| (format!("pkg{i}/package-lock.json"), "v1"))
            .collect();
        files.push(("package-lock.json".to_string(), "v1"));
        files.push((".gitignore".to_string(), "node_modules/\n"));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        wt_commit(&f, &refs);
        probe(&f, &["node_modules"]);
        let again = probe(&f, &["node_modules"]);
        assert!(!kept(&again, "node_modules"), "{again}");
    }
);

parity!(
    record_needs_a_git_checkout_on_the_box,
    covers = ["B-40"],
    tags = [],
    {
        let f = fx();
        wt_commit(&f, &[("a.txt", "a")]);
        crabbox::run_in(&f.w, &f.w.box_root().join("work"));
        let marker = f.w.box_root().join("command-ran");
        let o = rec(&f, "unit", &format!("touch '{}'", marker.display()));
        assert!(o.is_err());
        assert!(!marker.exists());
        assert_ne!(moor().runs(&f.w, &f.name)[0].exit, 0);
    }
);

fn path_without_hash_tools(w: &World) -> PathBuf {
    let dir = w.box_root().join("nohash-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let skip = ["sha256sum", "shasum", "openssl"];
    for src in ["/usr/bin", "/bin"] {
        for e in std::fs::read_dir(src).into_iter().flatten().flatten() {
            let name = e.file_name();
            if skip.iter().any(|s| name == *s) {
                continue;
            }
            let _ = std::os::unix::fs::symlink(e.path(), dir.join(&name));
        }
    }
    dir
}

parity!(
    record_without_a_hash_tool_keeps_nothing_and_still_runs,
    covers = ["B-47"],
    tags = [],
    {
        let f = fx();
        wt_commit(
            &f,
            &[
                ("package-lock.json", "v1"),
                (".gitignore", "node_modules/\n"),
            ],
        );
        let bin = path_without_hash_tools(&f.w);
        f.w.prepend_rules(
            "crabbox",
            [parity::fake::Rule::on(&["run"]).exec(format!("PATH='{}' {{post}}", bin.display()))],
        );
        probe(&f, &["node_modules"]);
        let again = probe(&f, &["node_modules"]);
        assert!(!kept(&again, "node_modules"), "{again}");
    }
);
