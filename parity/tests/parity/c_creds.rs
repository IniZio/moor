use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use parity::adapter::{EnvOpts, EnvReport, ExitClass, Outcome, RunOpts};
use parity::assert::{assert_never_called, assert_secret_absent};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

struct Fx {
    w: World,
    repo: Repo,
}

fn fx() -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    Fx { w, repo }
}

fn write_file(path: &Path, body: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

impl Fx {
    fn user(&self, doc: Value) {
        write_file(&self.w.config_path(), &doc.to_string(), 0o644);
    }

    fn repo_cfg(&self, doc: Value) {
        write_file(&self.repo.path.join(".moor.yaml"), &doc.to_string(), 0o644);
    }

    fn env_with(&self, opts: &EnvOpts) -> Outcome<EnvReport> {
        moor().env(&self.w, &self.repo.path, opts)
    }

    fn env(&self) -> Outcome<EnvReport> {
        self.env_with(&EnvOpts::default())
    }

    fn env_at(&self, location: &str) -> Outcome<EnvReport> {
        self.env_with(&EnvOpts {
            agent_location: Some(location.into()),
            ..Default::default()
        })
    }

    fn secret_file(&self, name: &str, body: &str) -> String {
        let path = self.w.root().join("secrets").join(name);
        write_file(&path, body, 0o600);
        path.to_string_lossy().into_owned()
    }

    fn secret(&self, tag: &str) -> String {
        let secret = format!("SECRET-{tag}-{}", self.w.task_name("v"));
        self.w.register_secret(&secret);
        secret
    }
}

fn assert_rejected(o: &Outcome<EnvReport>, what: &str) {
    assert_eq!(o.class, ExitClass::Failed, "{what}: {}", o.stderr);
    assert!(o.stdout.trim().is_empty(), "{what}: stdout {}", o.stdout);
    assert!(!o.stderr.trim().is_empty(), "{what}: silent failure");
}

fn assert_accepted(o: &Outcome<EnvReport>, what: &str) {
    assert_eq!(o.class, ExitClass::Ok, "{what}: {}", o.stderr);
    o.ok();
}

fn user_rejects(f: &Fx, docs: &[Value]) {
    for doc in docs {
        f.user(doc.clone());
        assert_rejected(&f.env(), &doc.to_string());
    }
}

fn repo_rejects(f: &Fx, docs: &[Value]) {
    for doc in docs {
        f.repo_cfg(doc.clone());
        assert_rejected(&f.env(), &doc.to_string());
    }
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
        assert_eq!(report.location, "host");

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
        assert_rejected(
            &f.env_at("box"),
            "env box location with GH_TOKEN credential",
        );
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

parity!(
    repo_agent_location_and_model,
    covers = ["C-11"],
    tags = [],
    {
        let f = fx();
        assert_eq!(f.env().ok().location, "host");
        f.repo_cfg(json!({ "agent": { "location": "box", "model": "haiku" } }));
        let o = f.env();
        assert_eq!(o.ok().location, "box");
        assert_eq!(o.ok().model.as_deref(), Some("haiku"));
        repo_rejects(
            &f,
            &[
                json!({ "agent": { "location": "cloud" } }),
                json!({ "agent": { "nope": 1 } }),
            ],
        );
    }
);

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
    box_credentials_gh_token_rejected_every_location,
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
            for location in ["host", "box"] {
                f.repo_cfg(json!({ "agent": { "location": location }, "box_credentials": form }));
                assert_rejected(&f.env(), &format!("{location} {form}"));
            }
        }
        f.repo_cfg(json!({ "box_credentials": ["GH_TOKEN"] }));
        assert_rejected(&f.env_at("box"), "flag override box");
    }
);

parity!(
    box_credentials_claude_names_rejected_for_box_agent,
    covers = ["C-15"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("t", "v");
        f.user(json!({ "credentials": { "OTHER": { "file": file } } }));
        let forms = [
            json!(["CLAUDE_CODE_OAUTH_TOKEN"]),
            json!(["ANTHROPIC_API_KEY"]),
            json!({ "OTHER": { "env": "ANTHROPIC_AUTH_TOKEN" } }),
            json!({ "OTHER": { "file_env": "CLAUDE_CODE_OAUTH_TOKEN" } }),
        ];
        for form in forms {
            f.repo_cfg(json!({ "agent": { "location": "box" }, "box_credentials": form }));
            assert_rejected(&f.env(), &format!("box {form}"));
            f.repo_cfg(json!({ "box_credentials": form }));
            let host = f.env();
            assert_eq!(host.class, ExitClass::Ok, "host {form}: {}", host.stderr);
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

fn plain_dir(f: &Fx) -> std::path::PathBuf {
    let dir = f.w.root().join("plain");
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn json_of(o: &Outcome<EnvReport>) -> Value {
    serde_json::from_str(&o.stdout).unwrap_or_else(|e| panic!("env stdout is not JSON: {e}"))
}

parity!(env_exit_classes, covers = ["C-24"], tags = [], {
    let f = fx();
    f.user(json!({ "credentials": { "GONE": { "file": "/nonexistent/path" } } }));
    let unresolved = f.env();
    let report = unresolved.ok();
    assert_eq!(
        report.credential("GONE").and_then(|c| c.resolves),
        Some(false)
    );

    let outside = moor().env(
        &f.w,
        &plain_dir(&f),
        &EnvOpts {
            set_env: vec![(
                "GIT_CEILING_DIRECTORIES".into(),
                f.w.root().to_string_lossy().into_owned(),
            )],
            ..Default::default()
        },
    );
    assert_rejected(&outside, "outside a git repo");

    let usage = f.env_at("cloud");
    assert_eq!(usage.class, ExitClass::Usage, "{}", usage.stderr);
    assert!(usage.stdout.trim().is_empty());
});

parity!(env_json_shape, covers = ["C-25"], tags = [], {
    let f = fx();
    let file = f.secret_file("c", "v");
    f.user(json!({ "credentials": { "C": { "file": file } } }));
    f.repo_cfg(json!({ "agent": { "model": "haiku" }, "egress": ["a.example.com"] }));
    let o = f.env();
    o.ok();
    let v = json_of(&o);
    for key in ["repo", "items", "agent", "github"] {
        assert!(v.get(key).is_some(), "missing top-level {key}");
    }
    assert!(v["agent"].get("location").is_some() && v["agent"].get("model").is_some());
    for key in ["identity", "app_id", "login", "mint", "resolved"] {
        assert!(v["github"].get(key).is_some(), "missing github.{key}");
    }
    assert!(v["github"]["mint"]["status"].is_string());
    for key in ["mode", "app_installed", "token_source", "error"] {
        assert!(
            v["github"]["resolved"].get(key).is_some(),
            "missing resolved.{key}"
        );
    }
    let kinds = [
        "identity",
        "credential",
        "tool",
        "setup",
        "mcp",
        "provision",
        "egress",
        "run",
        "box",
        "dotfiles",
    ];
    for item in v["items"].as_array().expect("items array") {
        let kind = item["kind"].as_str().expect("kind");
        assert!(kinds.contains(&kind), "unknown kind {kind}");
        assert!(["user", "repo", "default"].contains(&item["source"].as_str().expect("source")));
        assert!(item["name"].is_string());
    }
});

parity!(
    env_text_output_marks_resolution,
    covers = ["C-26"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("c", "v");
        f.user(json!({ "credentials": {
        "GOOD": { "file": file }, "BAD": { "file": "/nonexistent/path" }
    } }));
        let o = f.env_with(&EnvOpts {
            text: true,
            ..Default::default()
        });
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            serde_json::from_str::<Value>(&o.stdout).is_err(),
            "text mode printed JSON"
        );
        let lines: Vec<&str> = o.stdout.lines().collect();
        assert!(lines[0].starts_with("agent"), "first line: {}", lines[0]);
        let good = lines
            .iter()
            .find(|l| l.contains("GOOD"))
            .expect("GOOD line");
        let bad = lines.iter().find(|l| l.contains("BAD")).expect("BAD line");
        assert!(
            good.contains('\u{2713}') && !good.contains('\u{2717}'),
            "{good}"
        );
        assert!(
            bad.contains('\u{2717}') && !bad.contains('\u{2713}'),
            "{bad}"
        );
    }
);

parity!(env_item_order_is_fixed, covers = ["C-27"], tags = [], {
    let f = fx();
    let dots = f.w.root().join("dots");
    std::fs::create_dir_all(&dots).expect("mkdir");
    let file = f.secret_file("c", "v");
    f.user(json!({
        "git": { "name": "N", "email": "n@example.com" },
        "credentials": { "ZED": { "file": file }, "ALPHA": { "file": file }, "MID": { "file": file } },
        "mcp_servers": { "usrmcp": { "command": "npx" } },
        "box": { "provider": "sprites" },
        "dotfiles": { "dir": dots.to_string_lossy() },
    }));
    write_file(&f.repo.path.join(".env"), "A=1\n", 0o644);
    write_file(
        &f.repo.path.join(".mcp.json"),
        &json!({ "mcpServers": { "repomcp": {} } }).to_string(),
        0o644,
    );
    f.repo_cfg(json!({
        "tools": ["jq"], "setup": ["true"], "provision": [".env"],
        "run": { "env": { "CI": "1" }, "path": ["/opt/bin"] },
        "egress": ["a.example.com"], "credentials": ["ALPHA"], "box_credentials": ["MID"],
        "box": { "size": "medium" },
    }));
    let o = f.env_at("box");
    let items = &o.ok().items;
    let rank = |k: &str| {
        [
            "identity",
            "credential",
            "tool",
            "setup",
            "mcp",
            "provision",
            "run",
            "egress",
            "box",
            "dotfiles",
        ]
        .iter()
        .position(|x| *x == k)
        .unwrap_or_else(|| panic!("unexpected kind {k}"))
    };
    let ranks: Vec<usize> = items.iter().map(|i| rank(&i.kind)).collect();
    assert!(
        ranks.windows(2).all(|p| p[0] <= p[1]),
        "kinds out of order: {ranks:?}"
    );
    for k in [
        "identity",
        "credential",
        "tool",
        "setup",
        "mcp",
        "provision",
        "run",
        "egress",
        "box",
        "dotfiles",
    ] {
        assert!(items.iter().any(|i| i.kind == k), "no {k} item");
    }
    let creds: Vec<&str> = o
        .ok()
        .of_kind("credential")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(creds, ["ALPHA", "MID", "ZED"]);
    let mcp: Vec<&str> = o
        .ok()
        .of_kind("mcp")
        .iter()
        .map(|i| i.source.as_str())
        .collect();
    assert_eq!(mcp, ["user", "repo"]);
    let boxed: Vec<&str> = o
        .ok()
        .of_kind("box")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(boxed, ["provider", "size", "gh"]);
    let run: Vec<&str> = o
        .ok()
        .of_kind("run")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(run.len(), 2, "{run:?}");
});

parity!(
    env_credential_sources_and_union,
    covers = ["C-28"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("c", "v");
        f.user(json!({ "credentials": {
        "USERONLY": { "file": file }, "UNUSED": { "file": file }
    } }));
        f.repo_cfg(json!({
            "credentials": ["USERONLY", "REPOREF"], "box_credentials": ["BOXONLY"],
        }));
        let o = f.env();
        let r = o.ok();
        let user_only = r.credential("USERONLY").expect("USERONLY");
        assert_eq!(
            (user_only.source.as_str(), user_only.resolves),
            ("user", Some(true))
        );
        assert!(
            r.credential("UNUSED").is_some(),
            "every user credential is listed"
        );
        for name in ["REPOREF", "BOXONLY"] {
            let c = r
                .credential(name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(
                (c.source.as_str(), c.resolves),
                ("repo", Some(false)),
                "{name}"
            );
            assert!(c.error.is_some(), "{name} has no error");
        }
    }
);

parity!(env_delivery_labels, covers = ["C-29"], tags = [], {
    let f = fx();
    let file = f.secret_file("c", "v");
    f.user(json!({ "credentials": {
        "LISTED": { "file": file }, "PLAIN": { "file": file },
        "CLAUDE_CODE_OAUTH_TOKEN": { "file": file },
    } }));
    f.repo_cfg(json!({ "box_credentials": ["LISTED"] }));
    for location in ["host", "box"] {
        let o = f.env_at(location);
        let r = o.ok();
        let label = |n: &str| r.credential(n).and_then(|c| c.delivery.clone());
        assert_eq!(label("LISTED").as_deref(), Some("stdin/file"), "{location}");
        assert_eq!(label("PLAIN").as_deref(), Some("host only"), "{location}");
        let claude = if location == "box" {
            "proxy"
        } else {
            "host only"
        };
        assert_eq!(
            label("CLAUDE_CODE_OAUTH_TOKEN").as_deref(),
            Some(claude),
            "{location}"
        );
        assert!(r
            .items
            .iter()
            .all(|i| i.delivery.as_deref() != Some("injected")));
    }
});

parity!(
    env_agent_location_flag_previews_without_side_effects,
    covers = ["C-30", "C-31"],
    tags = [],
    {
        let f = fx();
        f.user(json!({ "box": { "provider": "sprites" } }));
        f.repo_cfg(json!({ "egress": ["a.example.com"] }));
        let host = f.env();
        assert_eq!(host.ok().location, "host");
        assert!(host.ok().item("box", "gh").is_none());
        let boxed = f.env_at("box");
        assert_eq!(boxed.ok().location, "box");
        let gh = boxed.ok().item("box", "gh").expect("box gh row");
        assert_eq!(gh.delivery.as_deref(), Some("proxy"));
        assert!(boxed.ok().items.iter().all(|i| i.name != "GH_TOKEN"));
        let host_egress = host.ok().of_kind("egress")[0].detail.clone();
        let box_egress = boxed.ok().of_kind("egress")[0].detail.clone();
        assert_ne!(
            host_egress, box_egress,
            "box location changes the egress row"
        );

        f.repo_cfg(json!({ "agent": { "location": "box" } }));
        assert_eq!(f.env().ok().location, "box");
        let flag_wins = f.env_at("host");
        assert_eq!(flag_wins.ok().location, "host");
        assert!(flag_wins.ok().item("box", "gh").is_none());
        assert!(f.w.all_calls().is_empty(), "env must not call any tool");
    }
);

parity!(env_egress_item_variants, covers = ["C-32"], tags = [], {
    let f = fx();
    let egress_of = |o: &Outcome<EnvReport>| {
        let r = o.ok();
        let items = r.of_kind("egress");
        assert_eq!(items.len(), 1, "exactly one egress row");
        (
            items[0].name.clone(),
            items[0].detail.clone().unwrap_or_default(),
        )
    };
    f.user(json!({ "box": { "provider": "sprites" } }));
    f.repo_cfg(json!({ "egress": ["a.example.com"] }));
    let (name, detail) = egress_of(&f.env());
    assert_eq!(name, "allow");
    assert!(detail.contains("a.example.com"), "{detail}");

    f.repo_cfg(json!({}));
    let (open_name, open_detail) = egress_of(&f.env());
    assert_eq!(open_name, "policy");

    f.user(json!({ "box": { "provider": "local-container" } }));
    let (none_name, none_detail) = egress_of(&f.env());
    assert_eq!(none_name, "policy");
    assert_ne!(
        open_detail, none_detail,
        "open vs unsupported must read differently"
    );

    f.repo_cfg(json!({ "egress": ["a.example.com", "b.example.com"] }));
    let (n_name, n_detail) = egress_of(&f.env());
    assert_eq!(n_name, "policy");
    assert_ne!(
        n_detail, none_detail,
        "unsupported with entries must say so"
    );
    assert!(n_detail.contains('2'), "{n_detail}");
});

parity!(
    env_enforce_note_on_namespace,
    covers = ["C-33"],
    tags = [],
    {
        let f = fx();
        for location in ["host", "box"] {
            f.user(json!({ "box": { "provider": "namespace-devbox" } }));
            let o = f.env_at(location);
            assert!(
                o.ok().item("egress", "enforce").is_some(),
                "{location}: note missing"
            );
            f.user(json!({ "box": { "provider": "namespace-devbox", "enforce_egress": true } }));
            assert!(
                f.env_at(location).ok().item("egress", "enforce").is_none(),
                "{location}"
            );
        }
        f.user(json!({ "box": { "provider": "sprites" } }));
        assert!(f.env().ok().item("egress", "enforce").is_none());
    }
);

parity!(
    env_proxy_domains_rejected_only_with_policy_provider,
    covers = ["C-34"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "egress": ["api.github.com"] }));
        f.user(json!({ "box": { "provider": "sprites" } }));
        assert_rejected(&f.env_at("box"), "sprites + box + api.github.com");
        f.repo_cfg(json!({ "egress": ["api.anthropic.com"] }));
        assert_rejected(&f.env_at("box"), "sprites + box + api.anthropic.com");
        f.user(json!({ "box": { "provider": "local-container" } }));
        assert_accepted(&f.env_at("box"), "local-container never checks");
    }
);

parity!(
    env_credential_source_semantics,
    covers = ["C-05"],
    tags = [],
    {
        let f = fx();
        let secret = f.secret("src");
        let good = f.secret_file("good", &format!("{secret}\r\n"));
        let blank = f.secret_file("blank", "  \n");
        let empty = f.secret_file("empty", "");
        write_file(&f.w.home().join("tilde/secret"), &secret, 0o600);
        let dir = f.w.root().join("secrets/adir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        f.user(json!({ "credentials": {
        "CMD_OK": { "cmd": format!("cat {good}") },
        "CMD_FAIL": { "cmd": format!("cat {good}; exit 3") },
        "CMD_SILENT": { "cmd": "true" },
        "CMD_BLANK": { "cmd": format!("cat {blank}") },
        "CMD_STDIN": { "cmd": "cat" },
        "FILE_OK": { "file": good },
        "FILE_TILDE": { "file": "~/tilde/secret" },
        "FILE_MISSING": { "file": "/nonexistent/secret" },
        "FILE_EMPTY": { "file": empty },
        "FILE_DIR": { "file": dir.to_string_lossy() },
        "ENV_OK": { "env": "PARITY_V_SET" },
        "ENV_UNSET": { "env": "PARITY_V_UNSET" },
        "ENV_EMPTY": { "env": "PARITY_V_EMPTY" },
    } }));
        let o = f.env_with(&EnvOpts {
            set_env: vec![
                ("PARITY_V_SET".into(), secret.clone()),
                ("PARITY_V_EMPTY".into(), String::new()),
            ],
            ..Default::default()
        });
        let r = o.ok();
        let want = [
            ("CMD_OK", true),
            ("CMD_FAIL", false),
            ("CMD_SILENT", false),
            ("CMD_BLANK", false),
            ("CMD_STDIN", false),
            ("FILE_OK", true),
            ("FILE_TILDE", true),
            ("FILE_MISSING", false),
            ("FILE_EMPTY", false),
            ("FILE_DIR", false),
            ("ENV_OK", true),
            ("ENV_UNSET", false),
            ("ENV_EMPTY", false),
        ];
        for (name, resolves) in want {
            let c = r
                .credential(name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(c.resolves, Some(resolves), "{name}: {:?}", c.error);
            assert_eq!(c.error.is_some(), !resolves, "{name} error presence");
        }
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    env_never_prints_credential_material,
    covers = ["C-06", "C-38"],
    tags = [],
    {
        let f = fx();
        let (sa, sb, sc) = (f.secret("a"), f.secret("b"), f.secret("c"));
        let marker = f.secret("argmarker");
        let a_file = f.secret_file("a", &sa);
        write_file(&f.w.home().join("secrets/b"), &sb, 0o600);
        f.user(json!({
        "credentials": {
            "A": { "cmd": format!("FOO=1 BAR=2 sh -c 'cat \"$0\"' {a_file} --marker={marker}") },
            "B": { "file": "~/secrets/b" },
            "C": { "env": "PARITY_V_C" },
        },
        "mcp_servers": {
            "m": { "command": "npx", "args": ["--tok=${A}"], "env": { "K": "${B}" } },
            "u": { "url": "http://h/?k=${C}", "headers": { "X": "${C}" } },
        },
    }));
        let opts = |text| EnvOpts {
            text,
            set_env: vec![("PARITY_V_C".into(), sc.clone())],
            ..Default::default()
        };
        let json_run = f.env_with(&opts(false));
        let text_run = f.env_with(&opts(true));
        let r = json_run.ok();
        assert_eq!(text_run.class, ExitClass::Ok, "{}", text_run.stderr);
        for (name, resolves) in [("A", true), ("B", true), ("C", true)] {
            assert_eq!(
                r.credential(name).and_then(|c| c.resolves),
                Some(resolves),
                "{name}"
            );
        }
        let detail = |n: &str| {
            r.credential(n)
                .and_then(|c| c.detail.clone())
                .unwrap_or_default()
        };
        assert!(
            detail("A").contains("sh") && !detail("A").contains("FOO"),
            "{}",
            detail("A")
        );
        assert!(!detail("A").contains(&a_file) && !detail("A").contains("marker"));
        assert!(detail("B").contains("~/secrets/b"), "{}", detail("B"));
        assert!(detail("C").contains("PARITY_V_C"), "{}", detail("C"));
        for out in [&json_run.stdout, &text_run.stdout] {
            for hidden in [&sa, &sb, &sc, &marker] {
                assert!(
                    !out.contains(hidden.as_str()),
                    "env output carries {hidden}"
                );
            }
        }
        for s in [&sa, &sb, &sc, &marker] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(env_github_status_block, covers = ["C-36"], tags = [], {
    let f = fx();
    let status = |f: &Fx| json_of(&f.env())["github"].clone();
    let none = status(&f);
    assert_eq!(none["identity"], "auto");
    assert!(none["app_id"].is_null());
    assert_eq!(none["mint"]["status"], "skipped");
    let auto_login = none["login"].clone();
    f.user(json!({ "github": { "identity": "bot" } }));
    let bot = status(&f);
    assert_eq!(bot["login"], "n/a");
    assert_ne!(bot["login"], auto_login);

    let store = f.w.state_dir().join("github");
    let (client_secret, refresh) = (f.secret("clientsecret"), f.secret("refresh"));
    write_file(
        &store.join("app.json"),
        &json!({ "app_id": "4242", "client_id": "Iv1.x", "client_secret": client_secret })
            .to_string(),
        0o600,
    );
    write_file(&store.join("app.pem"), "not a pem\n", 0o600);
    write_file(
        &store.join("user.json"),
        &json!({ "refresh_token": refresh }).to_string(),
        0o600,
    );
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let key = format!("url.{}.insteadOf", f.repo.bare.display());
    let unset =
        f.w.command("git")
            .args(["config", "--global", "--unset", &key])
            .status();
    assert!(unset.expect("git config").success());
    f.user(json!({ "github": { "identity": "you" } }));
    let stored = f.env();
    assert_eq!(stored.class, ExitClass::Ok, "{}", stored.stderr);
    let g = json_of(&stored)["github"].clone();
    assert_eq!(g["identity"], "you");
    assert_eq!(g["app_id"], "4242");
    assert!(
        g["login"] != "n/a" && g["login"] != auto_login,
        "{}",
        g["login"]
    );
    assert_eq!(g["mint"]["status"], "failed", "{g}");
    for s in [&client_secret, &refresh] {
        assert_secret_absent(&f.w, s, &["github/"]);
    }
});

parity!(env_user_mcp_items, covers = ["C-39"], tags = [], {
    let f = fx();
    let secret = f.secret("mcp");
    let file = f.secret_file("t", &secret);
    f.user(json!({
        "credentials": { "T": { "file": file }, "DEAD": { "file": "/nonexistent/x" } },
        "mcp_servers": {
            "cmdsrv": { "command": "npx", "args": ["--tok=${T}"] },
            "urlsrv": { "url": "http://h.example.com/mcp", "headers": { "A": "${T}" } },
            "broken": { "command": "x", "args": ["${DEAD}"] },
        },
    }));
    let o = f.env();
    let r = o.ok();
    let item = |n: &str| r.item("mcp", n).unwrap_or_else(|| panic!("{n} missing"));
    assert_eq!(item("cmdsrv").source, "user");
    assert_eq!(item("cmdsrv").detail.as_deref(), Some("npx"));
    assert_eq!(item("cmdsrv").resolves, Some(true));
    assert_eq!(
        item("urlsrv").detail.as_deref(),
        Some("http://h.example.com/mcp")
    );
    assert_eq!(item("broken").resolves, Some(false));
    assert!(item("broken")
        .error
        .as_deref()
        .is_some_and(|e| e.contains("DEAD")));
    assert_secret_absent(&f.w, &secret, &[]);
});

parity!(
    env_repo_mcp_items_follow_claude_approval,
    covers = ["C-40"],
    tags = [],
    {
        let f = fx();
        let repo = f.repo.path.clone();
        let servers = json!({ "mcpServers": { "a": {}, "b": {}, "c": {} } });
        assert!(
            f.env().ok().of_kind("mcp").is_empty(),
            "no .mcp.json, no items"
        );
        write_file(
            &repo.join(".mcp.json"),
            &json!({ "other": 1 }).to_string(),
            0o644,
        );
        assert!(
            f.env().ok().of_kind("mcp").is_empty(),
            "no mcpServers, no items"
        );

        write_file(&repo.join(".mcp.json"), &servers.to_string(), 0o644);
        write_file(
            &repo.join(".claude/settings.json"),
            &json!({ "enabledMcpjsonServers": ["a"] }).to_string(),
            0o644,
        );
        write_file(
            &repo.join(".claude/settings.local.json"),
            &json!({ "disabledMcpjsonServers": ["b"] }).to_string(),
            0o644,
        );
        let o = f.env();
        let r = o.ok();
        let state = |n: &str| {
            r.item("mcp", n)
                .map(|i| (i.source.as_str(), i.resolves, i.error.is_some()))
        };
        assert_eq!(state("a"), Some(("repo", Some(true), false)));
        assert_eq!(state("b"), Some(("repo", Some(false), true)));
        assert_eq!(state("c"), Some(("repo", Some(false), true)));
        assert_ne!(
            r.item("mcp", "b").unwrap().error,
            r.item("mcp", "c").unwrap().error
        );

        write_file(
            &repo.join(".claude/settings.local.json"),
            &json!({ "enableAllProjectMcpServers": true }).to_string(),
            0o644,
        );
        let all = f.env();
        assert_eq!(
            all.ok().item("mcp", "c").and_then(|i| i.resolves),
            Some(true)
        );

        write_file(&repo.join(".mcp.json"), "{ not json", 0o644);
        let bad = f.env();
        let items = bad.ok().of_kind("mcp");
        assert_eq!(items.len(), 1, "invalid JSON is one failed item");
        assert_eq!(items[0].resolves, Some(false));
        assert!(items[0].error.is_some());
    }
);

parity!(
    env_egress_text_never_counts_zero_entries,
    covers = [],
    tags = ["new_only(EnvEgressText)"],
    {
        let f = fx();
        f.user(json!({ "box": { "provider": "local-container" } }));
        let o = f.env_at("box");
        let items = o.ok().of_kind("egress");
        assert_eq!(items.len(), 1);
        let detail = items[0].detail.clone().unwrap_or_default();
        assert!(
            !detail.contains(" 0 "),
            "egress row counts zero entries: {detail}"
        );
    }
);

parity!(
    repo_credentials_gh_token_ref_is_a_hard_error,
    covers = [],
    tags = ["new_only(RepoGhTokenRef)"],
    {
        let f = fx();
        f.repo_cfg(json!({ "credentials": ["GH_TOKEN"] }));
        assert_rejected(&f.env(), "repo credentials GH_TOKEN");
        assert_rejected(&f.env_at("box"), "repo credentials GH_TOKEN, box");
    }
);

use parity::adapter::{EventKind, StartOpts, TaskState, TaskView};
use parity::fake::{claude, crabbox, herdr, Call, Rule};

fn flow_fakes(w: &World) {
    crabbox::install(w);
    herdr::install(w);
    claude::install(w);
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    let ssh_bin = w.tool_path("ssh");
    w.prepend_rules(
        "crabbox",
        [
            Rule::on(&["run"])
                .contains("MOOR_WORKDIR")
                .stdout("MOOR_WORKDIR={box}/work\n"),
            Rule::on(&["run"])
                .exec("{post}")
                .rewrite("/tmp/", "{box}/tmp/")
                .stderr("running on 192.0.2.10 {post}\n"),
            Rule::on(&["ssh"]).stdout(format!("'{}' 'box-host'\n", ssh_bin.display())),
            Rule::on(&["tunnel"])
                .stdout("{\"box\":1,\"host\":1}\n")
                .block(),
        ],
    );
}

fn wait_proxy_gone(w: &World) {
    let dir = w.state_dir().join("proxy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let any_json = std::fs::read_dir(&dir).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        });
        if !any_json {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("proxy supervisor did not exit after close");
}

fn start_task(f: &Fx, name: &str, goal: Option<&str>, location: Option<&str>) -> Outcome<TaskView> {
    let opts = StartOpts {
        goal: goal.map(str::to_string),
        location: location.map(str::to_string),
        ..Default::default()
    };
    moor().start(&f.w, &f.repo.path, name, &opts)
}

fn finish(f: &Fx, name: &str) {
    let closed = moor().close(&f.w, &f.repo.path, name, false);
    assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
    wait_proxy_gone(&f.w);
}

fn cred_dirs(w: &World) -> Vec<String> {
    std::fs::read_dir(w.box_root().join("tmp"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("moor-cred-"))
                .collect()
        })
        .unwrap_or_default()
}

fn probe_file(task: &TaskView, f: &Fx, name: &str) -> String {
    let wt = task.worktree.clone().unwrap_or_default();
    [
        PathBuf::from(wt).join(name),
        f.w.box_root().join("work").join(name),
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .unwrap_or_else(|| panic!("setup probe file {name} not written"))
    .trim()
    .to_string()
}

fn run_calls(w: &World) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some("run"))
        .collect()
}

const SETUP_PROBE: &str = concat!(
    "printf %s \"$TOK_V\" | wc -c > got-env; ",
    "wc -c < \"$KEYF_PATH\" > got-file; ",
    "stat -c %a \"$KEYF_PATH\" > got-filemode; ",
    "stat -c %a \"$(dirname \"$KEYF_PATH\")\" > got-dirmode"
);

fn creds_config(f: &Fx, tok: &str, keyf: &str) {
    let (t, k) = (f.secret_file("tok", tok), f.secret_file("keyf", keyf));
    f.user(json!({ "credentials": { "TOK": { "file": t }, "KEYF": { "file": k } } }));
}

fn pair_credentials() -> Value {
    json!({ "TOK": { "env": "TOK_V" }, "KEYF": { "file_env": "KEYF_PATH" } })
}

parity!(
    box_credentials_reach_setup_then_leave_no_trace,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "setup": [SETUP_PROBE], "box_credentials": pair_credentials() }));
        let name = f.w.task_name("setup");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let task = started.ok().clone();
        assert_eq!(task.state, TaskState::Working);
        assert_eq!(probe_file(&task, &f, "got-env"), tok.len().to_string());
        assert_eq!(probe_file(&task, &f, "got-file"), keyf.len().to_string());
        assert_eq!(probe_file(&task, &f, "got-filemode"), "600");
        assert_eq!(probe_file(&task, &f, "got-dirmode"), "700");

        let calls = f.w.all_calls();
        let setup_run = calls
            .iter()
            .find(|c| c.tool == "crabbox" && c.argv.iter().any(|a| a.contains("got-env")))
            .expect("setup run recorded");
        let delivered = calls
            .iter()
            .any(|c| c.tool == "ssh" && c.seq < setup_run.seq && c.stdin.contains(&tok));
        assert!(delivered, "credential reached the Box before setup ran");
        assert!(cred_dirs(&f.w).is_empty(), "{:?}", cred_dirs(&f.w));
        f.w.assert_no_new_tmp_creds();
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(
    box_credentials_failing_setup_still_removes_dir,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({
            "setup": ["test -n \"$TOK_V\"; exit 3"],
            "box_credentials": pair_credentials(),
        }));
        let name = f.w.task_name("badsetup");
        let started = start_task(&f, &name, None, None);
        let state = started.value.as_ref().map(|t| t.state.clone());
        assert_eq!(state, Some(TaskState::Working), "{}", started.stderr);
        assert!(f.w.calls("ssh").iter().any(|c| c.stdin.contains(&tok)));
        assert!(cred_dirs(&f.w).is_empty(), "{:?}", cred_dirs(&f.w));
        f.w.assert_no_new_tmp_creds();
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(
    setup_without_box_credentials_runs_unwrapped,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "setup": ["echo plain-setup"] }));
        let name = f.w.task_name("plain");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let setup = run_calls(&f.w)
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a.contains("plain-setup")))
            .expect("setup run");
        assert!(!setup.argv.iter().any(|a| a.contains("moor-cred")));
        assert!(
            f.w.calls("ssh").is_empty(),
            "no delivery without box_credentials"
        );
        finish(&f, &name);
    }
);

parity!(
    run_with_missing_or_unreadable_source_delivers_nothing,
    covers = ["C-128"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "box_credentials": ["TOK"] }));
        let name = f.w.task_name("nosrc");
        assert_eq!(start_task(&f, &name, None, None).class, ExitClass::Ok);
        let attempt = |f: &Fx| {
            moor().run(
                &f.w,
                &f.repo.path,
                &name,
                &["echo", "marker-no-source"],
                &RunOpts::default(),
            )
        };
        f.user(json!({}));
        let missing = attempt(&f);
        assert_eq!(missing.class, ExitClass::Failed, "{}", missing.stderr);
        f.user(json!({ "credentials": { "TOK": { "file": "/nonexistent/tok" } } }));
        let unreadable = attempt(&f);
        assert_eq!(unreadable.class, ExitClass::Failed, "{}", unreadable.stderr);
        assert!(f.w.calls("ssh").is_empty(), "nothing delivered");
        let ran = run_calls(&f.w)
            .iter()
            .any(|c| c.argv.iter().any(|a| a.contains("marker-no-source")));
        assert!(!ran, "command must not run without its credential");
        finish(&f, &name);
    }
);

parity!(
    run_hides_credential_wrapper_echo,
    covers = ["C-129"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "box_credentials": ["TOK"] }));
        let name = f.w.task_name("wrap");
        assert_eq!(start_task(&f, &name, None, None).class, ExitClass::Ok);
        for quiet in [false, true] {
            let opts = RunOpts {
                quiet,
                ..RunOpts::default()
            };
            let o = moor().run(
                &f.w,
                &f.repo.path,
                &name,
                &["echo", "hello-from-box"],
                &opts,
            );
            assert_eq!(o.class, ExitClass::Ok, "quiet={quiet}: {}", o.stderr);
            let all = format!("{}{}", o.stdout, o.stderr);
            for bad in ["__moor_creds", "trap ", "export ", "moor-cred"] {
                assert!(
                    !all.contains(bad),
                    "quiet={quiet}: output shows {bad}: {all}"
                );
            }
            assert!(f.w.calls("ssh").iter().any(|c| c.stdin.contains(&tok)));
        }
        assert!(cred_dirs(&f.w).is_empty());
        finish(&f, &name);
        assert_secret_absent(&f.w, &tok, &[]);
    }
);

parity!(
    gh_token_box_credential_delivers_nothing,
    covers = ["C-130"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("ghbox");
        let file = f.secret_file("gh", &secret);
        f.user(json!({ "credentials": { "OTHER": { "file": file } } }));
        f.repo_cfg(
            json!({ "box_credentials": { "OTHER": { "env": "GH_TOKEN" } }, "setup": ["true"] }),
        );
        let name = f.w.task_name("ghtok");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Failed, "{}", started.stdout);
        assert!(f.w.calls("ssh").is_empty(), "nothing delivered to a Box");
        assert!(run_calls(&f.w).is_empty(), "no Box script ran");
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    start_rejects_proxied_claude_box_credential_before_side_effects,
    covers = ["C-15"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("claude");
        let file = f.secret_file("claude", &secret);
        f.user(json!({ "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } } }));
        f.repo_cfg(json!({ "box_credentials": ["CLAUDE_CODE_OAUTH_TOKEN"] }));
        assert_accepted(&f.env(), "host location allows it");
        let name = f.w.task_name("claudebox");
        let started = start_task(&f, &name, Some("go"), Some("box"));
        let reason = serde_json::from_str::<Value>(&started.stdout)
            .ok()
            .and_then(|v| v["reason"].as_str().map(str::to_string))
            .unwrap_or_default();
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("CLAUDE_CODE_OAUTH_TOKEN"),
            "start must report the rejected dispatch: {} {}",
            started.stdout,
            started.stderr
        );
        assert_never_called(&f.w.all_calls(), "herdr", &["pane", "run"]);
        assert!(f.w.calls("ssh").is_empty(), "no delivery");
        assert!(
            f.w.calls("crabbox")
                .iter()
                .all(|c| c.argv[0] != "tunnel" && c.argv[0] != "ssh"),
            "no proxy or delivery side effect"
        );
        assert!(!f.w.state_dir().join("proxy").exists(), "no proxy state");
        assert_secret_absent(&f.w, &secret, &[]);
        if started.value.is_some() {
            finish(&f, &name);
        }
    }
);

parity!(
    box_agent_pane_receives_no_box_credentials,
    covers = [],
    tags = ["new_only(BoxCredsHostOnly)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({
            "agent": { "location": "box" },
            "setup": [SETUP_PROBE],
            "box_credentials": pair_credentials(),
        }));
        let name = f.w.task_name("pane");
        let started = start_task(&f, &name, Some("go"), None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let task = started.ok().clone();
        let setup_len = probe_file(&task, &f, "got-env");
        finish(&f, &name);

        assert_eq!(
            setup_len,
            tok.len().to_string(),
            "host-driven setup keeps its credential"
        );
        let calls = f.w.all_calls();
        let agent_dir = calls
            .iter()
            .filter(|c| c.tool == "ssh")
            .find_map(|c| {
                let cmd = c.argv.last()?;
                let at = cmd.find("proxy-nonce")?;
                cmd[..at].rsplit('\'').nth(1).map(str::to_string)
            })
            .expect("agent launch dir");
        for c in calls.iter().filter(|c| c.tool == "ssh") {
            let into_agent_dir = c.argv.last().is_some_and(|a| a.contains(&agent_dir));
            assert!(
                !(into_agent_dir && (c.stdin.contains(&tok) || c.stdin.contains(&keyf))),
                "agent launch dir received a box credential"
            );
        }
        for c in calls.iter().filter(|c| c.tool == "herdr") {
            let line = c.argv.join(" ");
            assert!(
                !line.contains("TOK_V") && !line.contains("KEYF_PATH"),
                "pane exports a credential"
            );
        }
    }
);

fn bind_listing(w: &World, wt: &Path, branch: &str) {
    let body = json!({ "result": { "worktrees": [{
        "branch": branch, "label": "bound", "open_workspace_id": "ws-bound", "path": wt,
    }] } });
    w.prepend_rules(
        "herdr",
        [Rule::on(&["worktree", "list"]).stdout(body.to_string())],
    );
}

fn started_ok(f: &Fx, name: &str, goal: Option<&str>) -> TaskView {
    let o = start_task(f, name, goal, None);
    assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
    o.ok().clone()
}

fn assert_rejected_start(f: &Fx, o: &Outcome<TaskView>, name: &str) {
    assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
    assert!(!o.stderr.trim().is_empty());
    let state = o.value.as_ref().map(|t| t.state.clone());
    assert_eq!(state, Some(TaskState::Rejected));
    assert_never_called(&f.w.all_calls(), "crabbox", &["warmup"]);
    assert_never_called(&f.w.all_calls(), "herdr", &["worktree", "create"]);
    let kinds: Vec<EventKind> = moor()
        .events(&f.w, name)
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&EventKind::Rejected), "{kinds:?}");
}

parity!(
    start_rejects_unresolved_credential_and_mcp_refs,
    covers = ["C-41", "C-136"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "credentials": ["MISSING"] }));
        let name = f.w.task_name("refcred");
        assert_rejected_start(&f, &start_task(&f, &name, None, None), &name);

        let f = fx();
        flow_fakes(&f.w);
        f.user(json!({
            "credentials": { "DEAD": { "file": "/nonexistent/x" } },
            "mcp_servers": { "srv": { "command": "x", "args": ["${DEAD}"] } },
        }));
        let name = f.w.task_name("mcpcred");
        assert_rejected_start(&f, &start_task(&f, &name, None, None), &name);

        let f = fx();
        flow_fakes(&f.w);
        write_file(
            &f.repo.path.join(".mcp.json"),
            &json!({ "mcpServers": { "a": {} } }).to_string(),
            0o644,
        );
        let name = f.w.task_name("mcpappr");
        assert_rejected_start(&f, &start_task(&f, &name, None, None), &name);
        assert!(
            !f.repo.path.join(".claude").exists(),
            "moor never writes the approval"
        );

        write_file(
            &f.repo.path.join(".claude/settings.local.json"),
            &json!({ "enabledMcpjsonServers": ["a"] }).to_string(),
            0o644,
        );
        let again = f.w.task_name("mcpok");
        started_ok(&f, &again, None);
        finish(&f, &again);
    }
);

parity!(
    start_ignores_unrelated_broken_credential,
    covers = [],
    tags = ["new_only(EnvPreflightScoped)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.user(json!({ "credentials": { "UNRELATED": { "file": "/nonexistent/x" } } }));
        let name = f.w.task_name("unrel");
        let o = start_task(&f, &name, None, None);
        let state = o.value.as_ref().map(|t| t.state.clone());
        assert_eq!(
            state,
            Some(TaskState::Working),
            "start rejected: {}",
            o.stderr
        );
        finish(&f, &name);
    }
);

parity!(
    start_failures_print_outcome_json_on_stdout,
    covers = [],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        crabbox::fail_quota(&f.w);
        let quota = start_task(&f, &f.w.task_name("quota"), None, None);
        assert_eq!(quota.class, ExitClass::Failed);
        assert!(
            quota.value.is_some(),
            "acquire failure prints the outcome: {}",
            quota.stdout
        );

        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "credentials": ["MISSING"] }));
        let rejected = start_task(&f, &f.w.task_name("rej"), None, None);
        assert_eq!(rejected.class, ExitClass::Failed);
        assert!(
            rejected.value.is_some(),
            "rejection prints the outcome: {}",
            rejected.stdout
        );

        let f = fx();
        flow_fakes(&f.w);
        let blank = f.secret_file("blank", "  \n");
        f.user(json!({ "credentials": { "TOK": { "file": blank } } }));
        f.repo_cfg(json!({ "setup": ["true"], "box_credentials": ["TOK"] }));
        let name = f.w.task_name("blank");
        let setup = start_task(&f, &name, None, None);
        assert!(
            setup.value.is_some(),
            "credential read failure prints the outcome: {}",
            setup.stdout
        );
        finish(&f, &name);
    }
);

parity!(
    checked_event_and_registry_carry_names_only,
    covers = ["C-42"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "credentials": ["TOK"], "box_credentials": pair_credentials() }));
        let name = f.w.task_name("checked");
        started_ok(&f, &name, None);
        let events = moor().events(&f.w, &name);
        let checked = EventKind::Other("checked".into());
        let mut seen = 0;
        for e in &events {
            let text = e.payload.to_string();
            assert!(
                !text.contains(&tok) && !text.contains(&keyf),
                "{:?} carries a value",
                e.kind
            );
            if e.kind == checked {
                let list = e.payload["credentials"]
                    .as_array()
                    .expect("credentials list");
                assert!(!list.is_empty(), "checked event lists no credentials");
                seen += 1;
                for c in list {
                    let keys: Vec<&String> = c.as_object().expect("entry").keys().collect();
                    assert!(keys
                        .iter()
                        .all(|k| ["name", "source", "resolves"].contains(&k.as_str())));
                }
            }
        }
        assert_eq!(seen, 1, "one checked event expected: {events:?}");
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

fn bg_call(w: &World) -> Call {
    w.calls("claude")
        .into_iter()
        .find(|c| c.argv.iter().any(|a| a == "--bg"))
        .expect("claude --bg dispatched")
}

fn mcp_files(w: &World) -> Vec<PathBuf> {
    std::fs::read_dir(w.state_dir().join("mcp"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn repo_snapshot(repo: &Path) -> Vec<(String, Vec<u8>)> {
    let out = std::process::Command::new("find")
        .arg(repo)
        .args(["-path", "*/.git", "-prune", "-o", "-type", "f", "-print"])
        .output()
        .expect("find");
    let mut files: Vec<(String, Vec<u8>)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| (l.to_string(), std::fs::read(l).unwrap_or_default()))
        .collect();
    files.sort();
    files
}

parity!(
    mcp_config_file_for_dispatched_host_agent,
    covers = ["C-132", "C-133", "C-134"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("mcp");
        let file = f.secret_file("mcp", &secret);
        f.user(json!({
            "credentials": { "T": { "file": file } },
            "mcp_servers": {
                "srv": { "command": "npx", "args": ["--tok=${T}"], "env": { "K": "${T}" } },
                "web": { "url": "http://h.example.com/${T}", "headers": { "X": "${T}" } },
            },
        }));
        write_file(
            &f.repo.path.join(".mcp.json"),
            &json!({ "mcpServers": { "own": {} } }).to_string(),
            0o644,
        );
        write_file(
            &f.repo.path.join(".claude/settings.local.json"),
            &json!({ "enableAllProjectMcpServers": true }).to_string(),
            0o644,
        );
        let before = repo_snapshot(&f.repo.path);
        let name = f.w.task_name("mcp");
        started_ok(&f, &name, Some("go"));
        let path = PathBuf::from(
            bg_call(&f.w)
                .flag("--mcp-config")
                .expect("--mcp-config flag"),
        );
        assert!(
            path.starts_with(f.w.state_dir().join("mcp")),
            "{}",
            path.display()
        );
        let mode = std::fs::metadata(&path)
            .expect("mcp file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let doc: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        let servers = &doc["mcpServers"];
        assert_eq!(servers["srv"]["args"][0], format!("--tok={secret}"));
        assert_eq!(servers["srv"]["env"]["K"], secret.as_str());
        assert_eq!(
            servers["web"]["url"],
            format!("http://h.example.com/{secret}")
        );
        assert_eq!(servers["web"]["headers"]["X"], secret.as_str());
        assert!(servers["srv"].get("url").is_none() && servers["web"].get("command").is_none());
        assert_secret_absent(&f.w, &secret, &["mcp/"]);
        assert_eq!(
            repo_snapshot(&f.repo.path),
            before,
            "moor writes nothing into the repo"
        );
        finish(&f, &name);
        assert!(mcp_files(&f.w).is_empty(), "mcp file survives close");
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    mcp_config_absent_without_servers_and_removed_on_failed_dispatch,
    covers = ["C-134"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let name = f.w.task_name("nomcp");
        started_ok(&f, &name, Some("go"));
        assert_eq!(bg_call(&f.w).flag("--mcp-config"), None);
        assert!(mcp_files(&f.w).is_empty());
        finish(&f, &name);

        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("mcpfail");
        let file = f.secret_file("mcp", &secret);
        f.user(json!({
            "credentials": { "T": { "file": file } },
            "mcp_servers": { "srv": { "command": "npx", "args": ["${T}"] } },
        }));
        f.w.prepend_rules(
            "claude",
            [Rule::any()
                .contains("--bg")
                .exec("cp '{flag:--mcp-config}' '{root}/mcp-seen'")
                .exit(1)
                .stderr("dispatch refused\n")],
        );
        let name = f.w.task_name("failmcp");
        let _ = start_task(&f, &name, Some("go"), None);
        let seen = std::fs::read_to_string(f.w.root().join("mcp-seen"))
            .expect("mcp config existed at dispatch time");
        assert!(seen.contains(&secret), "mcp config lacked the server args");
        assert!(
            mcp_files(&f.w).is_empty(),
            "failed dispatch leaves {:?}",
            mcp_files(&f.w)
        );
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    host_agent_dispatch_carries_no_credentials,
    covers = ["C-137", "C-138"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, claude_tok, mcp) = (f.secret("tok"), f.secret("claude"), f.secret("mcp"));
        let dots = f.w.root().join("dots");
        std::fs::create_dir_all(&dots).expect("mkdir");
        f.user(json!({
            "credentials": {
                "TOK": { "file": f.secret_file("tok", &tok) },
                "CLAUDE_CODE_OAUTH_TOKEN": { "file": f.secret_file("claude", &claude_tok) },
                "M": { "file": f.secret_file("mcp", &mcp) },
            },
            "mcp_servers": { "srv": { "command": "npx", "args": ["${M}"] } },
            "dotfiles": { "dir": dots.to_string_lossy() },
        }));
        f.repo_cfg(json!({ "box_credentials": ["TOK"] }));
        let name = f.w.task_name("posture");
        started_ok(&f, &name, Some("go"));
        let bg = bg_call(&f.w);
        assert_eq!(bg.flag("--name").as_deref(), Some(name.as_str()));
        assert_eq!(bg.flag("--permission-mode").as_deref(), Some("acceptEdits"));
        for flag in ["--model", "--setting-sources", "--allowedTools"] {
            assert!(bg.flag(flag).is_some(), "missing {flag}: {:?}", bg.argv);
        }
        let line = bg.argv.join(" ");
        assert!(!line.contains("--dangerously-skip-permissions"), "{line}");
        let tools = bg.flag("--allowedTools").expect("allowed tools");
        assert!(
            !tools.contains("git push") && !tools.contains("gh "),
            "{tools}"
        );
        assert!(
            bg.env
                .keys()
                .all(|k| !k.starts_with("ANTHROPIC_") && k != "CLAUDE_CODE_OAUTH_TOKEN"),
            "{:?}",
            bg.env.keys()
        );
        assert!(
            !bg.env_digests.contains_key("TOK"),
            "box_credential name TOK present in claude env"
        );
        for v in [&tok, &claude_tok, &mcp] {
            let d = parity::fake::digest(v);
            assert!(
                bg.env_digests.values().all(|x| *x != d),
                "a credential value reached the claude env"
            );
        }
        assert!(
            f.w.calls("ssh").is_empty(),
            "no delivery to a Box for a host agent"
        );
        assert_never_called(&f.w.all_calls(), "crabbox", &["tunnel"]);
        assert!(
            !f.w.state_dir().join("proxy").exists(),
            "host agents never use the proxy"
        );
        for s in [&tok, &claude_tok] {
            assert_secret_absent(&f.w, s, &[]);
        }
        assert_secret_absent(&f.w, &mcp, &["mcp/"]);
        finish(&f, &name);
    }
);

parity!(
    worktree_moor_yaml_wins_for_box_size_else_main,
    covers = ["C-09"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "box": { "size": "large" } }));
        let wt = f.w.root().join("wt-explicit");
        let status =
            f.w.command("git")
                .args(["worktree", "add", "-q", "-b", "wt-branch"])
                .arg(&wt)
                .current_dir(&f.repo.path)
                .status()
                .expect("git worktree add");
        assert!(status.success());
        bind_listing(&f.w, &wt, "wt-branch");
        let class_of = |name: &str| {
            let opts = StartOpts {
                worktree: Some(wt.clone()),
                ..Default::default()
            };
            let o = moor().start(&f.w, &f.repo.path, name, &opts);
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            let calls = f.w.calls("crabbox");
            let warm = calls
                .iter()
                .rfind(|c| c.argv[0] == "warmup")
                .expect("warmup");
            warm.flag("--class")
        };
        let fallback = f.w.task_name("wtmain");
        let main_class = class_of(&fallback);
        assert!(main_class.is_some());
        finish(&f, &fallback);
        write_file(
            &wt.join(".moor.yaml"),
            &json!({ "box": { "size": "medium" } }).to_string(),
            0o644,
        );
        let own = f.w.task_name("wtown");
        let own_class = class_of(&own);
        assert!(own_class.is_some());
        assert_ne!(own_class, main_class, "worktree size must win");
        finish(&f, &own);
    }
);

parity!(
    bound_worktree_config_drives_setup_and_box_credentials,
    covers = [],
    tags = ["new_only(WorktreeRepoConfig)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "setup": ["true"] }));
        let wt = f.w.root().join("wt-cfg");
        let status =
            f.w.command("git")
                .args(["worktree", "add", "-q", "-b", "wt-cfg-branch"])
                .arg(&wt)
                .current_dir(&f.repo.path)
                .status()
                .expect("git worktree add");
        assert!(status.success());
        bind_listing(&f.w, &wt, "wt-cfg-branch");
        write_file(
            &wt.join(".moor.yaml"),
            &json!({ "setup": [SETUP_PROBE], "box_credentials": pair_credentials() }).to_string(),
            0o644,
        );
        let name = f.w.task_name("wtcfg");
        let opts = StartOpts {
            worktree: Some(wt.clone()),
            ..Default::default()
        };
        let o = moor().start(&f.w, &f.repo.path, &name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let task = o.ok().clone();
        assert_eq!(probe_file(&task, &f, "got-env"), tok.len().to_string());
        finish(&f, &name);
    }
);

parity!(
    start_override_to_box_drops_proxy_domains_from_egress,
    covers = [],
    tags = ["new_only(EgressOverride)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        crabbox::install_preset(&f.w, crabbox::Preset::Sprites);
        f.user(json!({ "box": { "provider": "sprites" } }));
        f.repo_cfg(json!({ "egress": ["a.example.com"] }));
        let name = f.w.task_name("egovr");
        let o = start_task(&f, &name, Some("go"), Some("box"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let calls = f.w.calls("crabbox");
        let warm = calls
            .iter()
            .find(|c| c.argv[0] == "warmup")
            .expect("warmup");
        let allow = warm.flag("--sprites-network-allow").expect("allow list");
        let hosts: Vec<&str> = allow.split(',').collect();
        assert!(hosts.contains(&"a.example.com"));
        assert!(!hosts.contains(&"api.github.com") && !hosts.contains(&"api.anthropic.com"));
        finish(&f, &name);
    }
);
