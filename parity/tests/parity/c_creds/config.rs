use parity::adapter::{EnvOpts, ExitClass};
use parity::assert::assert_secret_absent;
use parity::parity;
use serde_json::json;

use super::support::*;
use crate::moor;

fn merge_repo_cfg(f: &Fx, doc: serde_json::Value) {
    let path = f.repo.path.join(".moor.yaml");
    let mut cur: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read repo cfg")).expect("repo cfg");
    cur.as_object_mut()
        .expect("object")
        .extend(doc.as_object().expect("object").clone());
    f.repo_cfg(cur);
}

parity!(config_path_precedence, covers = ["C-01"], tags = [], {
    let f = fx();
    let root = f.w.root().to_path_buf();
    let doc = |p: &str| json!({ "box": { "provider": p } }).to_string();
    write_file(&root.join("m.yaml"), &doc("namespace-devbox"), 0o644);
    write_file(&root.join("xdg/moor/config.yaml"), &doc("sprites"), 0o644);
    write_file(
        &root.join("hh/.config/moor/config.yaml"),
        &doc("ssh"),
        0o644,
    );
    let probe = |set: &[(&str, &str)], unset: &[&str]| {
        f.env_with(&EnvOpts {
            set_env: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            unset_env: unset.iter().map(|k| k.to_string()).collect(),
            ..Default::default()
        })
    };
    let m = root.join("m.yaml").to_string_lossy().into_owned();
    let xdg = root.join("xdg").to_string_lossy().into_owned();
    let hh = root.join("hh").to_string_lossy().into_owned();

    let all = probe(
        &[
            ("MOOR_CONFIG", &m),
            ("XDG_CONFIG_HOME", &xdg),
            ("HOME", &hh),
        ],
        &[],
    );
    assert_eq!(all.ok().provider(), Some("namespace-devbox"));
    let empty_moor = probe(
        &[
            ("MOOR_CONFIG", ""),
            ("XDG_CONFIG_HOME", &xdg),
            ("HOME", &hh),
        ],
        &[],
    );
    assert_eq!(empty_moor.ok().provider(), Some("sprites"));
    let empty_xdg = probe(&[("XDG_CONFIG_HOME", ""), ("HOME", &hh)], &["MOOR_CONFIG"]);
    assert_eq!(empty_xdg.ok().provider(), Some("ssh"));
    let home_only = probe(&[("HOME", &hh)], &["MOOR_CONFIG", "XDG_CONFIG_HOME"]);
    assert_eq!(home_only.ok().provider(), Some("ssh"));
    let none = probe(&[], &["MOOR_CONFIG", "XDG_CONFIG_HOME", "HOME"]);
    assert_rejected(&none, "no config location");
});

parity!(
    config_missing_file_defaults_unreadable_fails,
    covers = ["C-02"],
    tags = [],
    {
        let f = fx();
        let missing = f.env();
        let report = missing.ok();
        assert_eq!(report.provider(), Some("local-container"));
        assert_eq!(report.size(), Some("small"));

        let dir = f.w.root().join("config-is-a-dir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let unreadable = f.env_with(&EnvOpts {
            set_env: vec![("MOOR_CONFIG".into(), dir.to_string_lossy().into_owned())],
            ..Default::default()
        });
        assert_rejected(&unreadable, "unreadable config");
    }
);

parity!(
    config_unknown_keys_and_bad_yaml_rejected,
    covers = ["C-02"],
    tags = [],
    {
        let f = fx();
        user_rejects(
            &f,
            &[
                json!({ "hostz": 1 }),
                json!({ "box": { "nope": 1 } }),
                json!({ "github": { "nope": 1 } }),
                json!({ "reports": { "nope": 1 } }),
                json!({ "git": { "nope": 1 } }),
                json!({ "dotfiles": { "nope": 1 } }),
                json!({ "drivers": { "crabbox": { "nope": 1 } } }),
                json!({ "credentials": { "A": { "env": "X", "nope": 1 } } }),
                json!({ "mcp_servers": { "m": { "command": "x", "nope": 1 } } }),
            ],
        );
        write_file(&f.w.config_path(), "{{{ not yaml", 0o644);
        assert_rejected(&f.env(), "invalid yaml");
    }
);

parity!(
    user_gh_token_credential_rejected_everywhere,
    covers = ["C-03", "C-130"],
    tags = [],
    {
        let f = fx();
        let secret = f.secret("ghpat");
        let file = f.secret_file("ghtoken", &secret);
        f.user(json!({ "credentials": { "GH_TOKEN": { "file": file } } }));
        assert_rejected(&f.env(), "env with GH_TOKEN credential");
        let listed = moor().list(&f.w, &f.repo.path);
        assert_eq!(listed.class, ExitClass::Failed, "{}", listed.stderr);
        assert!(listed.stdout.trim().is_empty(), "{}", listed.stdout);
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(credential_entry_validation, covers = ["C-04"], tags = [], {
    let f = fx();
    let file = f.secret_file("ok", "v");
    user_rejects(
        &f,
        &[
            json!({ "credentials": { "bad name": { "file": file } } }),
            json!({ "credentials": { "A": {} } }),
            json!({ "credentials": { "A": { "file": file, "env": "X" } } }),
            json!({ "credentials": { "A": { "cmd": "   " } } }),
            json!({ "credentials": { "A": { "env": "" } } }),
        ],
    );
    f.user(json!({ "credentials": { "A-b_1": { "file": file } } }));
    assert_accepted(&f.env(), "valid credential name");
});

parity!(
    config_github_identity_and_box_values,
    covers = ["C-07", "C-08"],
    tags = [],
    {
        let f = fx();
        for id in ["you", "bot", "auto"] {
            f.user(json!({ "github": { "identity": id, "you_token_cmd": "gh auth token" } }));
            assert_eq!(f.env().ok().identity, id);
        }
        f.user(json!({}));
        assert_eq!(f.env().ok().identity, "auto");
        user_rejects(
            &f,
            &[
                json!({ "github": { "identity": "Bot" } }),
                json!({ "github": { "identity": "other" } }),
                json!({ "box": { "size": "huge" } }),
                json!({ "box": { "enforce_egress": "yes" } }),
            ],
        );
        for size in ["small", "medium", "large", "xlarge"] {
            f.user(json!({ "box": { "size": size, "provider": "custom-provider" } }));
            let o = f.env();
            assert_eq!(o.ok().size(), Some(size));
            assert_eq!(o.ok().provider(), Some("custom-provider"));
        }
        f.user(json!({ "box": { "enforce_egress": true } }));
        assert_accepted(&f.env(), "enforce_egress bool");
    }
);

parity!(
    repo_box_user_only_keys_and_size_precedence,
    covers = ["C-10"],
    tags = [],
    {
        let f = fx();
        repo_rejects(
            &f,
            &[
                json!({ "box": { "provider": "sprites" } }),
                json!({ "box": { "enforce_egress": true } }),
            ],
        );
        f.user(json!({ "box": { "size": "medium", "provider": "sprites" } }));
        f.repo_cfg(json!({}));
        assert_eq!(f.env().ok().size(), Some("medium"));
        f.repo_cfg(json!({ "box": { "size": "large" } }));
        let o = f.env();
        assert_eq!(o.ok().size(), Some("large"));
        assert_eq!(o.ok().provider(), Some("sprites"));
        f.repo_cfg(json!({ "box": { "size": "gigantic" } }));
        assert_rejected(&f.env(), "invalid repo size");
    }
);

parity!(repo_agent_model, covers = ["C-11"], tags = [], {
    let f = fx();
    f.repo_cfg(json!({ "agent": { "model": "haiku" } }));
    let o = f.env();
    assert_eq!(o.ok().model.as_deref(), Some("haiku"));
    repo_rejects(&f, &[json!({ "agent": { "nope": 1 } })]);
});

parity!(
    repo_egress_entry_validation,
    covers = ["C-12"],
    tags = [],
    {
        let f = fx();
        for bad in ["10.0.0.1", "localhost", "bad_char.com", "https://x.com", ""] {
            f.repo_cfg(json!({ "egress": [bad] }));
            assert_rejected(&f.env(), bad);
        }
        f.repo_cfg(json!({ "egress": ["Example.COM", "*.example.org"] }));
        assert_accepted(&f.env(), "valid egress entries");
    }
);

parity!(
    box_credentials_forms_and_validation,
    covers = ["C-13"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("t", "v");
        f.user(json!({ "credentials": {
        "NPM_TOKEN": { "file": file }, "KEY": { "file": file }, "OTHER": { "file": file }
    } }));
        let valid = [
            json!(["NPM_TOKEN"]),
            json!({ "NPM_TOKEN": null }),
            json!({ "NPM_TOKEN": {} }),
            json!({ "NPM_TOKEN": { "env": "NPM_AUTH" } }),
            json!({ "KEY": { "file_env": "KEY_PATH" }, "NPM_TOKEN": {} }),
        ];
        for v in valid {
            f.repo_cfg(json!({ "box_credentials": v }));
            assert_accepted(&f.env(), &v.to_string());
        }
        repo_rejects(
            &f,
            &[
                json!({ "box_credentials": ["bad name"] }),
                json!({ "box_credentials": { "NPM_TOKEN": { "env": "A", "file_env": "B" } } }),
                json!({ "box_credentials": { "NPM_TOKEN": { "env": "1BAD" } } }),
                json!({ "box_credentials": { "NPM_TOKEN": { "env": "PATH" } } }),
                json!({ "box_credentials": { "NPM_TOKEN": { "file_env": "PATH" } } }),
                json!({ "box_credentials": ["NPM_TOKEN", "NPM_TOKEN"] }),
                json!({ "box_credentials": { "NPM_TOKEN": { "nope": "X" } } }),
            ],
        );
    }
);

parity!(
    box_credentials_gh_token_rejected,
    covers = ["C-14"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("t", "v");
        f.user(json!({ "credentials": { "OTHER": { "file": file } } }));
        let forms = [
            json!(["GH_TOKEN"]),
            json!({ "GH_TOKEN": {} }),
            json!({ "OTHER": { "env": "GH_TOKEN" } }),
            json!({ "OTHER": { "file_env": "GH_TOKEN" } }),
        ];
        for form in forms {
            f.repo_cfg(json!({ "box_credentials": form }));
            assert_rejected(&f.env(), &format!("{form}"));
        }
    }
);

parity!(
    box_credentials_claude_names_rejected_for_box_agent,
    covers = ["C-15"],
    tags = [],
    {
        let f = box_fx();
        let file = f.secret_file("t", "v");
        f.user(json!({ "credentials": { "OTHER": { "file": file } } }));
        let forms = [
            json!(["CLAUDE_CODE_OAUTH_TOKEN"]),
            json!(["ANTHROPIC_API_KEY"]),
            json!({ "OTHER": { "env": "ANTHROPIC_AUTH_TOKEN" } }),
            json!({ "OTHER": { "file_env": "CLAUDE_CODE_OAUTH_TOKEN" } }),
        ];
        for form in forms {
            merge_repo_cfg(&f, json!({ "box_credentials": form }));
            assert_rejected(&f.env(), &format!("{form}"));
        }
    }
);

parity!(repo_record_keys_rejected, covers = ["C-16"], tags = [], {
    let f = fx();
    repo_rejects(
        &f,
        &[
            json!({ "record": { "keep": ["node_modules"] } }),
            json!({ "record": { "other": 1 } }),
        ],
    );
});

parity!(
    unknown_config_key_fails_parsing_naming_the_key_path,
    covers = ["C-16"],
    tags = ["new_only(StrictConfig)"],
    {
        let f = fx();
        f.repo_cfg(json!({ "egres": ["a.example.com"] }));
        let o = f.env();
        assert_rejected(&o, "unknown repo key");
        assert!(o.stderr.contains("egres"), "{}", o.stderr);
        f.repo_cfg(json!({}));
        f.user(json!({ "box": { "providr": "ssh" } }));
        let o = f.env();
        assert_rejected(&o, "unknown user key");
        assert!(o.stderr.contains("box.providr"), "{}", o.stderr);
        f.user(json!({}));
        for (doc, key) in [
            (json!({ "agent": { "location": "box" } }), "agent.location"),
            (
                json!({ "reports": { "backend": "local" } }),
                "reports.backend",
            ),
        ] {
            f.repo_cfg(doc);
            let o = f.env();
            assert_rejected(&o, key);
            assert!(o.stderr.contains(key), "{key}: {}", o.stderr);
        }
    }
);

parity!(
    git_identity_applies_to_worktree_and_box_commits,
    covers = ["C-17"],
    tags = ["new_only(GitIdentity)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.user(json!({ "git": { "name": "User Name" } }));
        f.repo_cfg(json!({
            "git": { "name": "Repo Name", "email": "repo@example.com" },
            "setup": [concat!(
                "mkdir -p bx && cd bx && git init -q && unset GIT_CONFIG_GLOBAL && ",
                "git commit -q --allow-empty -m x && ",
                "git log -1 --format=%an/%ae > ../got-author"
            )],
        }));
        let name = f.w.task_name("gitid");
        let task = started_ok(&f, &name, None);
        let wt = task.worktree.clone().expect("worktree");
        let commit = |dir: &str| {
            let st =
                f.w.command("git")
                    .env_remove("GIT_CONFIG_GLOBAL")
                    .args(["commit", "-q", "--allow-empty", "-m", "x"])
                    .current_dir(dir)
                    .status()
                    .expect("git commit");
            assert!(st.success(), "commit in {dir} failed");
            let out =
                f.w.command("git")
                    .env_remove("GIT_CONFIG_GLOBAL")
                    .args(["log", "-1", "--format=%an/%ae"])
                    .current_dir(dir)
                    .output()
                    .expect("git log");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(commit(&wt), "User Name/repo@example.com");
        assert_eq!(
            probe_file(&task, &f, "got-author"),
            "User Name/repo@example.com"
        );
        finish(&f, &name);
    }
);

parity!(
    git_identity_user_wins_repo_fills_gaps,
    covers = ["C-17"],
    tags = [],
    {
        let f = fx();
        f.user(json!({ "git": { "name": "User Name" } }));
        f.repo_cfg(json!({ "git": { "name": "Repo Name", "email": "repo@example.com" } }));
        let o = f.env();
        let r = o.ok();
        let name = r.item("identity", "user.name").expect("name item");
        let email = r.item("identity", "user.email").expect("email item");
        assert_eq!(name.source, "user");
        assert_eq!(email.source, "repo");
        assert!(name
            .detail
            .as_deref()
            .is_none_or(|d| d.contains("User Name")));
        f.user(json!({}));
        f.repo_cfg(json!({}));
        assert!(f.env().ok().of_kind("identity").is_empty());
    }
);

parity!(
    repo_list_entries_validation,
    covers = ["C-19"],
    tags = [],
    {
        let f = fx();
        repo_rejects(
            &f,
            &[
                json!({ "setup": [" "] }),
                json!({ "teardown": [""] }),
                json!({ "tools": [""] }),
                json!({ "provision": ["/etc/passwd"] }),
                json!({ "provision": ["../up"] }),
                json!({ "provision": [{ "path": "a", "mode": "move" }] }),
                json!({ "run": { "env": { "PATH": "x" } } }),
                json!({ "run": { "env": { "1BAD": "x" } } }),
                json!({ "run": { "path": ["bad\"quote"] } }),
                json!({ "run": { "path": [""] } }),
            ],
        );
        f.repo_cfg(json!({
            "setup": ["true"], "teardown": ["true"], "tools": ["jq"],
            "provision": [".env", { "path": "cfg", "mode": "symlink" }],
            "run": { "env": { "CI": "1" }, "path": ["/opt/bin"] }
        }));
        assert_accepted(&f.env(), "valid repo lists");
    }
);

parity!(user_mcp_server_validation, covers = ["C-20"], tags = [], {
    let f = fx();
    let file = f.secret_file("t", "v");
    let creds = json!({ "T": { "file": file } });
    user_rejects(
        &f,
        &[
            json!({ "credentials": creds, "mcp_servers": { "m": { "command": "x", "url": "http://h" } } }),
            json!({ "credentials": creds, "mcp_servers": { "m": { "args": ["a"] } } }),
            json!({ "credentials": creds, "mcp_servers": { "m": { "command": "x", "args": ["${NOPE}"] } } }),
            json!({ "credentials": creds, "mcp_servers": { "m": { "url": "http://h/${NOPE}" } } }),
            json!({ "credentials": creds, "mcp_servers": { "m": { "url": "http://h", "headers": { "A": "${NOPE}" } } } }),
            json!({ "credentials": creds, "mcp_servers": { "bad name": { "command": "x" } } }),
        ],
    );
    f.user(json!({ "credentials": creds, "mcp_servers": {
        "${NOPE}": { "command": "${NOPE}", "args": ["${T}"], "env": { "${NOPE}": "${T}" } },
    } }));
    assert_rejected(&f.env(), "ref in server name");
    f.user(json!({ "credentials": creds, "mcp_servers": {
        "m": { "command": "${NOPE}", "args": ["${T}"], "env": { "K": "${T}" } },
        "u": { "url": "http://h/${T}", "headers": { "X": "${T}" } },
    } }));
    assert_accepted(&f.env(), "refs only in checked positions");
});

parity!(
    driver_limits_dotfiles_and_reports_values,
    covers = ["C-21", "C-22", "C-23"],
    tags = [],
    {
        let f = fx();
        user_rejects(
            &f,
            &[
                json!({ "drivers": { "crabbox": { "max_boxes": 0 } } }),
                json!({ "drivers": { "crabbox": { "ssh_port": 0 } } }),
                json!({ "drivers": { "crabbox": { "ssh_port": 70000 } } }),
                json!({ "dotfiles": { "dir": "/abs", "git": "https://example.com/d.git" } }),
                json!({ "dotfiles": { "dir": "/abs", "ref": "main" } }),
                json!({ "dotfiles": { "dir": "relative/dir" } }),
                json!({ "dotfiles": { "git": "https://example.com/d.git", "install": "../x" } }),
                json!({ "dotfiles": { "git": "https://example.com/d.git", "install": "/abs" } }),
                json!({ "reports": { "backend": "s3" } }),
            ],
        );
        let dots = f.w.root().join("dots");
        std::fs::create_dir_all(&dots).expect("mkdir");
        f.user(json!({
        "drivers": { "crabbox": { "max_boxes": 2, "ssh_port": 2222, "sleep_after_minutes": 5 } },
        "dotfiles": { "dir": dots.to_string_lossy(), "install": "install.sh" },
        "reports": { "backend": "agentic-artifacts" },
    }));
        let o = f.env();
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.ok().of_kind("dotfiles").is_empty());
        f.user(json!({ "reports": { "backend": "local" } }));
        assert_accepted(&f.env(), "local reports backend");
    }
);
