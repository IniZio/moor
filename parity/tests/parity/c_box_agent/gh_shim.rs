use super::*;

struct Shim {
    script: PathBuf,
    repo: PathBuf,
    curl_log: PathBuf,
    delivered: String,
}

fn shim(b: &Bx, name: &str) -> Shim {
    let l = launched(b, name, "shim");
    let delivered = l.shim.expect("shim delivered");
    let dir = b.w.root().join("shim");
    let script = dir.join("gh");
    write_file(&script, &delivered, 0o700);
    let curl_log = dir.join("curl.log");
    let wrapper = format!(
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\nexec /usr/bin/curl \"$@\"\n",
        curl_log.display()
    );
    write_file(&dir.join("bin/curl"), &wrapper, 0o700);
    let repo = dir.join("checkout");
    std::fs::create_dir_all(&repo).expect("checkout dir");
    for args in [
        &["init", "-q", "-b", "feature/x"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let st =
            b.w.command("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .expect("git");
        assert!(st.success(), "git {args:?}");
    }
    Shim {
        script,
        repo,
        curl_log,
        delivered,
    }
}

struct Sh {
    code: i32,
    out: String,
    err: String,
}

impl Shim {
    fn invoke(&self, b: &Bx, args: &[&str], env: &[(String, String)]) -> Sh {
        let base = self.script.parent().expect("shim dir");
        let path = format!("{}:/usr/bin:/bin", base.join("bin").display());
        let mut cmd = b.w.command("sh");
        cmd.env_clear()
            .env("PATH", path)
            .env("HOME", b.w.home())
            .envs(env.iter().cloned())
            .current_dir(&self.repo)
            .arg(&self.script)
            .args(args);
        let out = cmd.output().expect("run shim");
        Sh {
            code: out.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&out.stdout).into_owned(),
            err: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, b: &Bx, args: &[&str], extra: &[(&str, &str)]) -> Sh {
        let mut env = vec![
            ("MOOR_GH_API".to_string(), b.gh.url()),
            ("MOOR_GH_REPO".to_string(), "acme/widget".to_string()),
        ];
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        self.invoke(b, args, &env)
    }

    fn without(&self, b: &Bx, args: &[&str], drop: &str) -> Sh {
        let env: Vec<(String, String)> = [
            ("MOOR_GH_API".to_string(), b.gh.url()),
            ("MOOR_GH_REPO".to_string(), "acme/widget".to_string()),
        ]
        .into_iter()
        .filter(|(k, _)| k != drop)
        .collect();
        self.invoke(b, args, &env)
    }

    fn curl_urls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.curl_log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("http"))
            .map(str::to_string)
            .collect()
    }
}

fn pulls(b: &Bx) -> String {
    format!("{}/pulls", b.fx.repo_path())
}

fn pr_reply(b: &Bx) {
    let body = json!({ "html_url": "https://github.com/acme/widget/pull/7", "number": 7 });
    b.gh.route("POST", &pulls(b), vec![Reply::json(201, &body)]);
}

fn sent(b: &Bx, method: &str, path: &str) -> Vec<parity::github::Request> {
    b.gh.requests_to(method, path)
}

fn parsed(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

parity!(
    box_gh_shim_supports_only_pr_commands_and_validates_flags_before_any_request,
    covers = ["C-112", "C-113"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("surface");
        let s = shim(&b, &name);
        assert!(
            !s.delivered.contains("GH_TOKEN"),
            "shim never reads a token"
        );
        let refused: [&[&str]; 7] = [
            &["repo", "view"],
            &["api", "/user"],
            &["auth", "token"],
            &["pr", "merge", "1"],
            &[],
            &["pr"],
            &["pr", "close", "1"],
        ];
        for args in refused {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
            assert!(r.err.contains("supports only"), "{args:?}: {}", r.err);
        }
        for drop in ["MOOR_GH_API", "MOOR_GH_REPO"] {
            let r = s.without(&b, &["pr", "list"], drop);
            assert_eq!(r.code, 2, "{drop}: {}", r.err);
            assert!(r.err.contains("MOOR_GH_API"), "{}", r.err);
        }
        let flag_errors: [(&[&str], &str); 4] = [
            (&["pr", "create", "--title"], "needs a value"),
            (&["pr", "create", "--bogus", "x"], "unsupported flag"),
            (&["pr", "view", "1", "2"], "unexpected argument"),
            (&["pr", "view", "abc"], "bad pull request number"),
        ];
        for (args, want) in flag_errors {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
            assert!(r.err.contains(want), "{args:?}: {}", r.err);
        }
        assert!(
            b.gh.requests().iter().all(|r| !r.path.contains("/pulls")),
            "no request was made"
        );
        assert!(s.curl_urls().is_empty());

        pr_reply(&b);
        let forms: [&[&str]; 5] = [
            &["pr", "create", "--title", "T1", "--body", "B1"],
            &[
                "pr", "create", "-t", "T2", "-b", "B2", "-B", "dev", "-H", "feat2", "-d",
            ],
            &[
                "pr",
                "create",
                "--title=T3",
                "--body=B3",
                "--base=rel",
                "--head=feat3",
            ],
            &[
                "pr", "create", "--title", "T4", "--body", "B4", "--draft", "--base", "x",
                "--head", "y",
            ],
            &[
                "pr",
                "create",
                "-R",
                "acme/widget",
                "--title",
                "T5",
                "--body",
                "B5",
            ],
        ];
        for args in forms {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
        }
        let reqs = sent(&b, "POST", &pulls(&b));
        let got: Vec<(String, String, bool)> = reqs
            .iter()
            .map(|r| {
                let j = r.json();
                let f = |k: &str| j[k].as_str().unwrap_or_default().to_string();
                (f("title"), f("base"), j["draft"].as_bool().unwrap_or(false))
            })
            .collect();
        assert_eq!(got[0], ("T1".into(), "main".into(), false));
        assert_eq!(got[1], ("T2".into(), "dev".into(), true));
        assert_eq!(got[2], ("T3".into(), "rel".into(), false));
        assert_eq!(got[3], ("T4".into(), "x".into(), true));
        assert_eq!(reqs[2].json()["head"], "feat3");
        assert_eq!(reqs[1].json()["body"], "B2");

        let body = json!({ "title": "T", "state": "open", "html_url": "U", "number": 5 });
        b.gh.route("GET", &format!("{}/5", pulls(&b)), vec![Reply::ok(&body)]);
        let views: [&[&str]; 2] = [
            &["pr", "view", "5", "--json", "title,state"],
            &["pr", "view", "--json", "5"],
        ];
        for args in views {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
            assert_eq!(parsed(&r.out), Some(body.clone()), "{args:?}: {}", r.out);
        }
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_refuses_another_repo_or_owner_and_accepts_the_task_repo,
    covers = ["C-114", "REQ-39"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("guard");
        let s = shim(&b, &name);
        b.gh.route("GET", &pulls(&b), vec![Reply::ok(&json!([]))]);
        pr_reply(&b);
        let accepted: [&[&str]; 3] = [
            &["pr", "list", "--repo", "acme/widget"],
            &["pr", "list", "-R", "ACME/Widget"],
            &["pr", "list", "--repo=acme/widget"],
        ];
        for args in accepted {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
        }
        let seen = sent(&b, "GET", &pulls(&b)).len();
        assert_eq!(seen, 3);
        let r = s.run(&b, &["pr", "list", "--repo", "other/repo"], &[]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.contains("does not match the Task repo") && r.err.contains("acme/widget"),
            "{}",
            r.err
        );
        let evil = [
            "pr",
            "create",
            "--title",
            "t",
            "--body",
            "b",
            "--head",
            "evil:feat",
        ];
        let r = s.run(&b, &evil, &[]);
        assert_eq!(r.code, 2);
        assert!(r.err.contains("does not match the Task repo"), "{}", r.err);
        assert_eq!(
            sent(&b, "GET", &pulls(&b)).len(),
            seen,
            "refusals made no request"
        );
        assert!(sent(&b, "POST", &pulls(&b)).is_empty());

        let unattended = [
            "pr",
            "create",
            "--repo",
            "acme/widget",
            "--head",
            "acme:feat",
            "--title",
            "t",
            "--body",
            "b",
        ];
        let ok = s.run(&b, &unattended, &[]);
        assert_eq!(ok.code, 0, "{}", ok.err);
        assert_eq!(ok.out.trim(), "https://github.com/acme/widget/pull/7");
        let posted = sent(&b, "POST", &pulls(&b));
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].json()["head"], "acme:feat");
        let plain = [
            "pr", "create", "--head", "feat", "--title", "t", "--body", "b",
        ];
        let plain = s.run(&b, &plain, &[]);
        assert_eq!(plain.code, 0, "{}", plain.err);

        let moved = [("MOOR_GH_REPO_CANONICAL", "neworg/Gadget")];
        let canon = s.run(&b, &["pr", "list", "--repo", "neworg/gadget"], &moved);
        assert_eq!(canon.code, 0, "{}", canon.err);
        let head = [
            "pr",
            "create",
            "--head",
            "neworg:feat",
            "--title",
            "t",
            "--body",
            "b",
        ];
        let head = s.run(&b, &head, &moved);
        assert_eq!(head.code, 0, "{}", head.err);
        let bad = s.run(&b, &["pr", "list", "--repo", "neworg/gadget"], &[]);
        assert_eq!(bad.code, 2, "canonical name is accepted only when exported");
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_pr_create_escapes_json_and_defaults_head_and_base,
    covers = ["C-115"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("create");
        let s = shim(&b, &name);
        pr_reply(&b);
        let r = s.run(&b, &["pr", "create", "--title", "only a title"], &[]);
        assert_eq!(r.code, 2);
        assert!(r.err.contains("needs --title and --body"), "{}", r.err);
        assert!(sent(&b, "POST", &pulls(&b)).is_empty());

        let title = "say \"hi\" \\ back\tslash";
        let body = "line one\r\nline \"two\"\n\\end";
        let r = s.run(&b, &["pr", "create", "--title", title, "--body", body], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(r.out, "https://github.com/acme/widget/pull/7\n");
        let j = sent(&b, "POST", &pulls(&b))[0].json();
        assert_eq!(j["title"], title);
        assert_eq!(j["body"], body);
        assert_eq!(j["head"], "feature/x");
        assert_eq!(j["base"], "main");
        assert_eq!(j["draft"], false);

        let st =
            b.w.command("git")
                .current_dir(&s.repo)
                .args([
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/trunk",
                ])
                .status()
                .expect("git");
        assert!(st.success());
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(sent(&b, "POST", &pulls(&b))[1].json()["base"], "trunk");

        let bare = json!({ "number": 9 });
        b.gh.reroute("POST", &pulls(&b), vec![Reply::json(201, &bare)]);
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(
            r.out.contains("\"number\"") && r.out.contains('9'),
            "raw response without html_url: {}",
            r.out
        );
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_view_list_and_comment_talk_to_the_pull_routes,
    covers = ["C-116"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("read");
        let s = shim(&b, &name);
        let one = json!({ "number": 5, "title": "Fix it", "state": "open", "html_url": "U5" });
        b.gh.route("GET", &format!("{}/5", pulls(&b)), vec![Reply::ok(&one)]);
        let r = s.run(&b, &["pr", "view", "5"], &[]);
        let want = "title: Fix it\nstate: open\nurl: U5\n";
        assert_eq!((r.code, r.out.as_str()), (0, want), "{}", r.err);
        let raw = s.run(&b, &["pr", "view", "5", "--json"], &[]);
        assert_eq!(parsed(&raw.out), Some(one.clone()));

        let rows = json!([
            { "number": 1, "title": "First", "state": "open", "html_url": "U1", "user": { "id": 99 } },
            { "number": 2, "title": "Second", "state": "open", "html_url": "U2", "user": { "id": 98 } },
        ]);
        b.gh.route("GET", &pulls(&b), vec![Reply::ok(&rows)]);
        let r = s.run(&b, &["pr", "list"], &[]);
        let want = "#1 First\n#2 Second\n";
        assert_eq!((r.code, r.out.as_str()), (0, want), "{}", r.err);
        let raw = s.run(&b, &["pr", "list", "--json", "number,title"], &[]);
        assert_eq!(parsed(&raw.out), Some(rows.clone()));

        let cur = s.run(&b, &["pr", "view"], &[]);
        let want = "title: First\nstate: open\nurl: U1\n";
        assert_eq!((cur.code, cur.out.as_str()), (0, want), "{}", cur.err);
        let urls = s.curl_urls();
        let want = format!(
            "{}/repos/acme/widget/pulls?head=acme:feature/x&state=open",
            b.gh.url()
        );
        assert!(urls.contains(&want), "{urls:?}");

        b.gh.reroute("GET", &pulls(&b), vec![Reply::ok(&json!([]))]);
        let none = s.run(&b, &["pr", "view"], &[]);
        assert_eq!(none.code, 1);
        assert!(none.err.contains("no pull request found"), "{}", none.err);

        let comments = format!("{}/issues/7/comments", b.fx.repo_path());
        let reply = Reply::json(201, &json!({ "html_url": "C7" }));
        b.gh.route("POST", &comments, vec![reply]);
        let r = s.run(&b, &["pr", "comment", "7", "--body", "looks \"good\""], &[]);
        assert_eq!((r.code, r.out.as_str()), (0, "C7\n"), "{}", r.err);
        let sent_body = sent(&b, "POST", &comments)[0].json();
        assert_eq!(sent_body, json!({ "body": "looks \"good\"" }));
        let incomplete: [&[&str]; 2] = [&["pr", "comment", "--body", "x"], &["pr", "comment", "7"]];
        for args in incomplete {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
        }
        assert_eq!(sent(&b, "POST", &comments).len(), 1);
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_reports_http_failures_on_stderr_and_never_sends_credentials,
    covers = ["C-117", "C-118"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("http");
        let s = shim(&b, &name);
        b.gh.route("GET", &pulls(&b), vec![Reply::message(403, "forbidden")]);
        let r = s.run(&b, &["pr", "list"], &[]);
        assert_eq!(r.code, 1);
        assert!(
            r.err.contains("forbidden") && r.out.is_empty(),
            "{} / {}",
            r.out,
            r.err
        );
        let invalid = Reply::json(422, &json!({ "message": "Validation Failed" }));
        b.gh.route("POST", &pulls(&b), vec![invalid]);
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 1);
        assert!(
            r.err.contains("Validation Failed") && r.out.is_empty(),
            "{} / {}",
            r.out,
            r.err
        );

        pr_reply(&b);
        let token = "ghp_PARITY_HOST_TOKEN";
        let env = [("GH_TOKEN", token), ("GITHUB_TOKEN", token)];
        let calls: [&[&str]; 3] = [
            &["pr", "list"],
            &["pr", "create", "--title", "t", "--body", "b"],
            &["pr", "comment", "3", "--body", "c"],
        ];
        for args in calls {
            s.run(&b, args, &env);
        }
        let reqs: Vec<_> =
            b.gh.requests()
                .into_iter()
                .filter(|r| r.path.contains("/pulls") || r.path.contains("/issues"))
                .collect();
        assert!(reqs.len() >= 5, "{reqs:?}");
        for r in &reqs {
            assert!(r.path.starts_with("/repos/acme/widget/"), "{}", r.path);
            assert!(
                r.header("authorization").is_none(),
                "credential header on {}",
                r.path
            );
            let headers = format!("{:?}", r.headers);
            assert!(!headers.contains(token) && !r.body.contains(token));
        }
        let base = b.gh.url();
        let urls = s.curl_urls();
        assert!(urls.iter().all(|u| u.starts_with(&base)), "{urls:?}");
        assert!(!s.delivered.contains("GH_TOKEN") && !s.delivered.contains("Authorization"));
        finish(&b, &name);
    }
);
