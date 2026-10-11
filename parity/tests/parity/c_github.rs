use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parity::adapter::github_ops::{GithubSetupOpts, GithubStore};
use parity::adapter::{EnvOpts, EventKind, ExitClass, Outcome, StartOpts};
use parity::assert::{assert_never_called, assert_pure_json, assert_secret_absent};
use parity::fake::{claude, crabbox, herdr, Rule};
use parity::github::{FakeGithub, Reply, RepoFixture, Request, TEST_KEY_PEM};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

struct Gx {
    w: World,
    repo: Repo,
    gh: FakeGithub,
    fx: RepoFixture,
}

fn gx() -> Gx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let gh = FakeGithub::start();
    gh.attach(&w);
    let fx = RepoFixture::new("acme", "widget");
    for s in [
        &fx.user_access,
        &fx.user_refresh,
        &fx.scoped_token,
        &fx.bot_token,
        &fx.probe_token,
    ] {
        w.register_secret(s);
    }
    for s in ["JUNKKEYMARKER", "ghp_HOSTTOKEN", "ghr_NEW"] {
        w.register_secret(s);
    }
    if let Some(body) = TEST_KEY_PEM.lines().nth(1) {
        w.register_secret(body);
    }
    Gx { w, repo, gh, fx }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn write_file(path: &Path, body: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn snap(o: &Outcome<GithubStore>) -> &GithubStore {
    o.value.as_ref().expect("store snapshot")
}

fn seq(reqs: &[Request]) -> Vec<String> {
    reqs.iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

fn assert_subseq(reqs: &[Request], want: &[&str]) {
    let got = seq(reqs);
    let mut it = got.iter();
    for w in want {
        assert!(
            it.any(|g| g == w),
            "request {w} missing or out of order in {got:?}"
        );
    }
}

fn count(reqs: &[Request], method: &str, path: &str) -> usize {
    reqs.iter()
        .filter(|r| r.method == method && r.path == path)
        .count()
}

fn permissions_write() -> Value {
    json!({ "contents": "write", "pull_requests": "write", "workflows": "write" })
}

impl Gx {
    fn secret(&self, tag: &str) -> String {
        let secret = format!("SECRET-{tag}-{}", self.w.task_name("v"));
        self.w.register_secret(&secret);
        secret
    }

    fn user(&self, doc: Value) {
        write_file(&self.w.config_path(), &doc.to_string(), 0o644);
    }

    fn identity(&self, identity: &str) {
        self.user(json!({ "github": { "identity": identity } }));
    }

    fn git(&self, args: &[&str]) {
        let st = self
            .w
            .command("git")
            .current_dir(&self.repo.path)
            .args(args)
            .status()
            .expect("git");
        assert!(st.success(), "git {args:?}");
    }

    fn github_origin(&self) {
        let key = format!("url.{}.insteadOf", self.repo.bare.display());
        let st = self
            .w
            .command("git")
            .args(["config", "--global", "--unset", &key])
            .status()
            .expect("git config");
        assert!(st.success());
    }

    fn key(&self, mode: u32) -> PathBuf {
        let path = self.w.root().join(format!("app-key-{mode:o}.pem"));
        write_file(&path, TEST_KEY_PEM, mode);
        path
    }

    fn opts(&self, secret: &str) -> GithubSetupOpts {
        GithubSetupOpts {
            app_id: Some("4242".into()),
            client_id: Some(self.fx.client_id.clone()),
            key: Some(self.key(0o600)),
            repo: Some("acme/widget".into()),
            secret_env: Some(secret.to_string()),
            ..Default::default()
        }
    }

    fn setup_with(&self, opts: &GithubSetupOpts) -> Outcome<GithubStore> {
        moor().github_setup(&self.w, &self.repo.path, opts)
    }

    fn setup(&self) -> String {
        let secret = self.secret("clientsecret");
        let o = self.setup_with(&self.opts(&secret));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        secret
    }

    fn login(&self) -> Outcome<GithubStore> {
        moor().github_login(&self.w, &self.repo.path)
    }

    fn store(&self) -> GithubStore {
        GithubStore::read(&self.w)
    }

    fn wipe_store(&self) {
        let _ = std::fs::remove_dir_all(self.w.state_dir().join("github"));
    }

    fn seed_user(&self, refresh: &str, access: Option<(&str, u64)>) {
        let mut doc = json!({ "refresh_token": refresh });
        if let Some((token, expires_at)) = access {
            doc["access_token"] = json!(token);
            doc["access_expires_at"] = json!(expires_at);
        }
        let path = self.w.state_dir().join("github/user.json");
        write_file(&path, &doc.to_string(), 0o600);
    }

    fn env_json(&self) -> Value {
        let o = moor().env(&self.w, &self.repo.path, &EnvOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        serde_json::from_str(&o.stdout).expect("env json")
    }

    fn mint(&self) -> Value {
        self.env_json()["github"]["mint"].clone()
    }

    fn mint_status(&self) -> String {
        self.mint()["status"].as_str().unwrap_or("").to_string()
    }

    fn mint_error(&self) -> String {
        self.mint()["error"].as_str().unwrap_or("").to_string()
    }

    fn resolved(&self) -> Value {
        self.env_json()["github"]["resolved"].clone()
    }

    fn installation(&self, status: u16) {
        let reply = match status {
            200 => Reply::ok(&json!({ "id": self.fx.installation_id })),
            s => Reply::message(s, "Not Found"),
        };
        let path = format!("{}/installation", self.fx.repo_path());
        self.gh.reroute("GET", &path, vec![reply]);
    }

    fn gh_token_calls(&self) -> usize {
        self.w
            .calls("gh")
            .iter()
            .filter(|c| c.argv == ["auth", "token"])
            .count()
    }

    fn no_gh_fallback(&self) {
        assert_never_called(&self.w.all_calls(), "gh", &["auth"]);
    }
}

parity!(
    github_setup_requires_app_client_and_key,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let full = g.opts("s3cret");
        let cases = [
            GithubSetupOpts {
                app_id: None,
                ..full.clone()
            },
            GithubSetupOpts {
                client_id: None,
                ..full.clone()
            },
            GithubSetupOpts {
                key: None,
                ..full.clone()
            },
        ];
        for opts in cases {
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Usage, "{}", o.stderr);
            assert!(!snap(&o).exists(), "usage error created the store");
        }
        assert!(g.gh.requests().is_empty());
    }
);

parity!(
    github_setup_accepts_repo_forms_and_rejects_others,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let forms = [
            "acme/widget",
            "acme/widget.git",
            "https://github.com/acme/widget",
            "https://github.com/acme/widget.git",
            "git@github.com:acme/widget.git",
        ];
        for form in forms {
            let mut opts = g.opts("s3cret");
            opts.repo = Some(form.into());
            opts.json = true;
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Ok, "{form}: {}", o.stderr);
            let v = assert_pure_json(&o.stdout);
            assert_eq!(v["repo"], "acme/widget", "{form}");
            assert_eq!(v["installation_id"], g.fx.installation_id, "{form}");
        }
        let probe = "/repos/acme/widget/installation";
        assert_eq!(count(&g.gh.requests(), "GET", probe), forms.len());
        let before = g.gh.requests().len();
        for bad in ["widget", "https://gitlab.com/acme/widget", "acme/widget/x"] {
            let mut opts = g.opts("s3cret");
            opts.repo = Some(bad.into());
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Failed, "{bad}");
        }
        assert_eq!(g.gh.requests().len(), before, "bad repo reached GitHub");
    }
);

parity!(
    github_setup_default_repo_is_origin,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.github_origin();
        let mut opts = g.opts("s3cret");
        opts.repo = None;

        g.git(&["remote", "remove", "origin"]);
        let none = g.setup_with(&opts);
        assert_eq!(none.class, ExitClass::Failed, "{}", none.stderr);
        assert!(!snap(&none).exists());

        g.git(&["remote", "add", "origin", "https://example.com/acme/widget"]);
        let foreign = g.setup_with(&opts);
        assert_eq!(foreign.class, ExitClass::Failed, "{}", foreign.stderr);
        assert!(!snap(&foreign).exists());
        assert!(g.gh.requests().is_empty());

        g.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/widget",
        ]);
        let ok = g.setup_with(&opts);
        assert_eq!(ok.class, ExitClass::Ok, "{}", ok.stderr);
        let probe = g.gh.requests_to("GET", "/repos/acme/widget/installation");
        assert_eq!(probe.len(), 1);
    }
);

parity!(
    github_setup_client_secret_intake_never_argv,
    covers = ["C-44"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let (env_secret, stdin_secret) = (g.secret("env"), g.secret("stdin"));
        let stored = |g: &Gx| g.store().app.expect("app.json")["client_secret"].clone();

        let mut opts = g.opts(&env_secret);
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), env_secret);

        opts.secret_env = None;
        opts.secret_stdin = Some(format!("  {stdin_secret}  \nSECOND-LINE-IGNORED\n"));
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), stdin_secret);

        opts.secret_env = Some(env_secret.clone());
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), env_secret, "env must win over stdin");

        let empties = [
            (Some(""), None),
            (None, Some("")),
            (None, Some("   \n")),
            (Some("  "), Some("ignored\n")),
        ];
        for (env, stdin) in empties {
            g.wipe_store();
            let mut o = g.opts("x");
            o.secret_env = env.map(str::to_string);
            o.secret_stdin = stdin.map(str::to_string);
            let r = g.setup_with(&o);
            assert_eq!(r.class, ExitClass::Failed, "{env:?} {stdin:?}");
            assert!(snap(&r).app.is_none(), "empty secret stored an App");
        }

        g.wipe_store();
        let mut flagged = g.opts(&env_secret);
        flagged.extra_args = vec!["--client-secret".into(), "FLAGVALUE".into()];
        let r = g.setup_with(&flagged);
        assert_eq!(r.class, ExitClass::Usage, "{}", r.stderr);
        assert!(!snap(&r).exists());

        g.setup_with(&g.opts(&env_secret));
        for s in [&env_secret, &stdin_secret] {
            assert_secret_absent(&g.w, s, &["github/"]);
        }
    }
);

parity!(
    github_setup_refuses_loose_key_files,
    covers = ["C-45"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let secret = g.secret("client");
        let marker = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        for mode in [0o640, 0o604, 0o644, 0o660, 0o666] {
            let mut opts = g.opts(&secret);
            opts.key = Some(g.key(mode));
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Failed, "mode {mode:o}");
            assert!(!snap(&o).exists(), "mode {mode:o} created the store");
            assert!(!o.stdout.contains(marker) && !o.stderr.contains(marker));
            assert!(!o.stderr.contains(&secret));
        }
        assert!(g.gh.requests().is_empty(), "loose key reached GitHub");
        for mode in [0o600, 0o400] {
            let mut opts = g.opts(&secret);
            opts.key = Some(g.key(mode));
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Ok, "mode {mode:o}: {}", o.stderr);
            assert_eq!(snap(&o).dir_mode, Some(0o700));
        }
        assert_secret_absent(&g.w, marker, &["github/"]);
    }
);

parity!(
    github_store_layout_modes_and_parse_errors,
    covers = ["C-46"],
    tags = [],
    {
        let g = gx();
        g.github_origin();
        g.gh.script_repo(&g.fx);
        g.identity("you");
        let github_dir = g.w.state_dir().join("github");
        std::fs::create_dir_all(&github_dir).expect("mkdir");
        std::fs::set_permissions(&github_dir, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");

        let secret = g.setup();
        let s = g.store();
        assert_eq!(s.dir_mode, Some(0o700), "dir mode is enforced");
        assert_eq!(s.pem.as_deref(), Some(TEST_KEY_PEM.as_bytes()));
        assert_eq!(s.slug.as_deref(), Some(g.fx.slug.as_str()));
        let app = s.app.expect("app.json");
        assert_eq!(app["app_id"], "4242");
        assert_eq!(app["client_id"], g.fx.client_id);
        assert_eq!(app["client_secret"], secret);

        let login = g.login();
        assert_eq!(login.class, ExitClass::Ok, "{}", login.stderr);
        assert_eq!(g.mint_status(), "ok");
        let s = g.store();
        for name in ["app.pem", "app.json", "app.slug", "user.json", "user.lock"] {
            assert_eq!(s.file_mode(name), Some(0o600), "{name}");
        }
        assert!(s.leftover_tmp().is_empty(), "{:?}", s.leftover_tmp());
        assert_eq!(s.dir_mode, Some(0o700));

        write_file(&github_dir.join("app.json"), "{ \"x\": TAILMARKER", 0o600);
        let broken = g.login();
        assert_eq!(broken.class, ExitClass::Failed);
        assert!(broken.stderr.contains("app.json"), "{}", broken.stderr);
        assert!(!broken.stderr.contains("TAILMARKER") && !broken.stdout.contains("TAILMARKER"));

        let marker = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        assert_secret_absent(&g.w, marker, &["github/"]);
        assert_secret_absent(&g.w, &g.fx.user_refresh, &["github/"]);
    }
);

parity!(
    github_setup_flow_order_and_outputs,
    covers = ["C-47"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let mut opts = g.opts("s3cret");
        opts.json = true;
        let o = g.setup_with(&opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let v = assert_pure_json(&o.stdout);
        let expected_store = g.w.state_dir().join("github");
        assert_eq!(v["store"], expected_store.to_string_lossy().as_ref());
        assert_eq!(v["app_id"], "4242");
        assert_eq!(v["repo"], "acme/widget");
        assert_eq!(v["installation_id"], g.fx.installation_id);
        let reqs = g.gh.requests();
        assert_eq!(
            seq(&reqs),
            ["GET /app", "GET /repos/acme/widget/installation"]
        );
        assert!(reqs.iter().all(|r| r.jwt().is_some()), "App JWT on both");
        assert_eq!(snap(&o).slug.as_deref(), Some("moor-bot"));

        opts.json = false;
        let text = g.setup_with(&opts);
        assert_eq!(text.class, ExitClass::Ok);
        assert!(!text.stdout.trim().is_empty());

        g.wipe_store();
        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let no_slug = g.setup_with(&opts);
        assert_eq!(no_slug.class, ExitClass::Ok, "slug lookup is best effort");
        assert!(snap(&no_slug).slug.is_none());
    }
);

parity!(
    github_setup_not_installed_keeps_app_and_names_slug,
    covers = ["C-48"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let probe = "/repos/acme/widget/installation";
        let denied = "Resource not accessible: does not have access to the given target";
        let cases = [
            Reply::message(404, "Not Found"),
            Reply::message(403, denied),
        ];
        for reply in cases {
            g.wipe_store();
            g.gh.reroute("GET", probe, vec![reply]);
            let o = g.setup_with(&g.opts("s3cret"));
            assert_eq!(o.class, ExitClass::Failed);
            assert!(snap(&o).app.is_some(), "App must stay saved");
            assert!(o.stderr.contains("moor-bot"), "{}", o.stderr);
        }

        g.wipe_store();
        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(snap(&o).app.is_some());
        assert!(!o.stderr.contains("moor-bot"));

        g.wipe_store();
        g.gh.reroute(
            "GET",
            "/app",
            vec![Reply::ok(&json!({ "slug": "moor-bot" }))],
        );
        g.gh.reroute("GET", probe, vec![Reply::message(500, "boom")]);
        let other = g.setup_with(&g.opts("s3cret"));
        assert_eq!(other.class, ExitClass::Failed);
        assert!(snap(&other).app.is_some());
        assert!(other.stderr.contains("500"), "{}", other.stderr);
        assert!(!other.stderr.contains("installations/new"));
    }
);

parity!(
    github_app_jwt_claims_and_key_secrecy,
    covers = ["C-49"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let before = now();
        g.setup();
        let after = now();
        let reqs = g.gh.requests();
        assert_eq!(reqs.len(), 2);
        for req in &reqs {
            let (header, claims) = req.jwt().expect("Bearer JWT");
            assert_eq!(header["alg"], "RS256");
            assert_eq!(claims["iss"].to_string().trim_matches('"'), "4242");
            let (iat, exp) = (
                claims["iat"].as_u64().unwrap(),
                claims["exp"].as_u64().unwrap(),
            );
            assert!(iat + 60 >= before && iat + 60 <= after, "iat {iat}");
            assert_eq!(exp - iat, 600, "exp follows now+540, iat now-60");
        }

        g.gh.clear_requests();
        g.wipe_store();
        let junk = "not a pem JUNKKEYMARKER\n";
        let key = g.w.root().join("junk.pem");
        write_file(&key, junk, 0o600);
        let mut opts = g.opts("s3cret");
        opts.key = Some(key);
        let o = g.setup_with(&opts);
        assert_eq!(o.class, ExitClass::Failed);
        assert!(g.gh.requests().is_empty(), "bad key reached GitHub");
        assert!(!o.stderr.contains("JUNKKEYMARKER") && !o.stdout.contains("JUNKKEYMARKER"));
        assert_secret_absent(&g.w, "JUNKKEYMARKER", &["github/"]);
    }
);

parity!(
    github_http_client_headers_and_urls,
    covers = ["C-50"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.w.set_env("MOOR_GITHUB_API", &format!("{}/", g.gh.url()));
        g.w.set_env("MOOR_GITHUB_WEB", &format!("{}/", g.gh.url()));
        g.setup();
        let reqs = g.gh.requests();
        assert_eq!(reqs[0].path, "/app", "trailing slash is trimmed");
        for req in &reqs {
            assert_eq!(req.header("user-agent"), Some("moor"));
            assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
            assert_eq!(req.header("x-github-api-version"), Some("2022-11-28"));
            assert_eq!(req.header("content-type"), Some("application/json"));
            assert!(req.bearer().is_some());
        }
        g.gh.clear_requests();
        assert_eq!(g.login().class, ExitClass::Ok);
        let web = g.gh.requests_to("POST", "/login/device/code");
        assert_eq!(web.len(), 1);
        assert_eq!(web[0].header("accept"), Some("application/json"));
        assert_eq!(web[0].header("user-agent"), Some("moor"));
        assert!(web[0].header("authorization").is_none());
        assert_eq!(web[0].json(), json!({ "client_id": g.fx.client_id }));
    }
);

parity!(
    github_http_errors_name_endpoint_status_and_hide_secrets,
    covers = ["C-50"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let probe = "/repos/acme/widget/installation";
        let long = "x".repeat(300);
        g.gh.reroute("GET", probe, vec![Reply::message(500, &long)]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stderr.contains(&format!("GET {probe}")), "{}", o.stderr);
        assert!(o.stderr.contains("500"));
        assert!(o.stderr.contains(&"x".repeat(200)), "reason is shown");
        assert!(!o.stderr.contains(&"x".repeat(201)), "reason is truncated");

        let leaky = json!({ "message": "nope", "token": "ghs_LEAKME", "private_key": "PEMLEAK" });
        g.gh.reroute("GET", probe, vec![Reply::json(401, &leaky)]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stderr.contains("401"));
        for leak in ["ghs_LEAKME", "PEMLEAK"] {
            assert!(!o.stderr.contains(leak) && !o.stdout.contains(leak));
        }

        g.installation(200);
        g.setup();
        let flow = json!({ "error": "device_flow_disabled", "error_description": "turn it on" });
        g.gh.reroute("POST", "/login/device/code", vec![Reply::json(400, &flow)]);
        let o = g.login();
        assert_eq!(o.class, ExitClass::Failed);
        for part in [
            "POST /login/device/code",
            "400",
            "device_flow_disabled",
            "turn it on",
        ] {
            assert!(o.stderr.contains(part), "{part}: {}", o.stderr);
        }
        assert!(snap(&o).user.is_none());
    }
);

parity!(
    github_login_device_flow_saves_only_refresh,
    covers = ["C-51"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let client_secret = g.setup();
        let stale = g.secret("staleaccess");
        g.seed_user("ghr_STALE", Some((&stale, now() + 99_999)));
        let poll = [
            Reply::oauth_error("authorization_pending"),
            Reply::ok(&json!({ "error": "slow_down", "interval": 0 })),
            Reply::ok(&g.fx.tokens_json()),
        ];
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "device_code",
            poll.to_vec(),
        );
        g.gh.clear_requests();

        let o = g.login();
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            o.stderr.contains("PARI-TY00"),
            "user code shown: {}",
            o.stderr
        );
        assert!(o.stderr.contains("https://github.com/login/device"));
        assert!(!o.stdout.trim().is_empty());

        let reqs = g.gh.requests();
        assert_eq!(seq(&reqs)[0], "POST /login/device/code");
        assert_eq!(reqs[0].json(), json!({ "client_id": g.fx.client_id }));
        let polls = g.gh.requests_to("POST", "/login/oauth/access_token");
        assert_eq!(polls.len(), 3, "pending and slow_down keep polling");
        for p in &polls {
            assert_eq!(
                p.json(),
                json!({
                    "client_id": g.fx.client_id,
                    "device_code": "dc_parity",
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                })
            );
        }
        let user = snap(&o).user.clone().expect("user.json");
        assert_eq!(user["refresh_token"], g.fx.user_refresh);
        assert!(user.get("access_token").is_none(), "cached access dropped");
        assert!(user.get("access_expires_at").is_none());
        assert_eq!(snap(&o).file_mode("user.json"), Some(0o600));
        let expires = user["expires_at"].as_u64().expect("refresh expiry");
        assert!(expires.abs_diff(now() + 15_552_000) < 120, "{expires}");
        assert_secret_absent(&g.w, &g.fx.user_access, &[]);
        assert_secret_absent(&g.w, &stale, &[]);
        assert_secret_absent(&g.w, &g.fx.user_refresh, &["github/"]);
        assert_secret_absent(&g.w, &client_secret, &["github/"]);
    }
);

parity!(github_login_failure_modes, covers = ["C-51"], tags = [], {
    let g = gx();
    g.gh.script_repo(&g.fx);
    let no_app = g.login();
    assert_eq!(no_app.class, ExitClass::Failed);
    assert!(
        g.gh.requests().is_empty(),
        "login without an App hit GitHub"
    );

    g.setup();
    let terminal = ["access_denied", "expired_token", "incorrect_device_code"];
    for error in terminal {
        let reply = Reply::oauth_error(error);
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "device_code",
            vec![reply],
        );
        let o = g.login();
        assert_eq!(o.class, ExitClass::Failed, "{error}");
        assert!(o.stderr.contains(error), "{error}: {}", o.stderr);
        assert!(snap(&o).user.is_none(), "{error} wrote user.json");
    }

    let expired = json!({
        "device_code": "dc_parity", "user_code": "PARI-TY00",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 0, "interval": 0,
    });
    g.gh.reroute("POST", "/login/device/code", vec![Reply::ok(&expired)]);
    g.gh.clear_requests();
    let o = g.login();
    assert_eq!(o.class, ExitClass::Failed);
    assert!(snap(&o).user.is_none());
    assert!(g
        .gh
        .requests_to("POST", "/login/oauth/access_token")
        .is_empty());
});

parity!(
    github_has_no_refresh_loop_subcommand,
    covers = ["C-52"],
    tags = [],
    {
        let g = gx();
        let o = moor().github_raw(&g.w, &g.repo.path, &["refresh-loop"]);
        assert_eq!(o.class, ExitClass::Usage, "{}", o.stderr);
        assert!(g.gh.requests().is_empty());
    }
);

fn app_you_world() -> Gx {
    let g = gx();
    g.github_origin();
    g.gh.script_repo(&g.fx);
    g
}

parity!(
    app_you_mint_refreshes_rotates_and_scopes_by_id,
    covers = ["C-53", "C-54", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let refresh = Reply::ok(&fx.tokens("ghr_NEW", 28800));
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "refresh_token",
            vec![refresh],
        );
        let client_secret = g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        let reqs = g.gh.requests();
        let token_path = "/login/oauth/access_token";
        let scoped_path = format!("/applications/{}/token/scoped", fx.client_id);
        let want = [
            format!("POST {token_path}"),
            "GET /repos/acme/widget".to_string(),
            format!("POST {scoped_path}"),
        ];
        assert_subseq(&reqs, &want.iter().map(String::as_str).collect::<Vec<_>>());

        let refresh_req = &g.gh.requests_to("POST", token_path)[0];
        assert_eq!(
            refresh_req.json(),
            json!({
                "client_id": fx.client_id,
                "client_secret": client_secret,
                "grant_type": "refresh_token",
                "refresh_token": "ghr_OLD",
            })
        );
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.user_access.as_str()));
        let scoped = &g.gh.requests_to("POST", &scoped_path)[0];
        assert_eq!(
            scoped.basic().as_deref(),
            Some(format!("{}:{client_secret}", fx.client_id).as_str())
        );
        assert_eq!(
            scoped.json(),
            json!({
                "access_token": fx.user_access,
                "target_id": fx.owner_id,
                "repository_ids": [fx.repo_id],
                "permissions": permissions_write(),
            })
        );

        let s = g.store();
        let user = s.user.clone().expect("user.json");
        assert_eq!(user["refresh_token"], "ghr_NEW", "rotated token saved");
        assert_eq!(user["access_token"], fx.user_access);
        let at = user["access_expires_at"].as_u64().expect("access expiry");
        assert!(at.abs_diff(now() + 28800) < 120, "{at}");
        assert_eq!(s.file_mode("user.json"), Some(0o600));
        assert_eq!(s.file_mode("user.lock"), Some(0o600));

        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        let again = g.gh.requests();
        assert_eq!(count(&again, "POST", token_path), 0, "cached access reused");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.user_access.as_str()));
        assert_eq!(
            count(&again, "POST", &scoped_path),
            1,
            "scoped token is not cached"
        );

        assert_secret_absent(&g.w, &fx.scoped_token, &[]);
        for s in [&client_secret, &fx.user_access, "ghr_NEW"] {
            assert_secret_absent(&g.w, s, &["github/"]);
        }
    }
);

parity!(
    app_you_refresh_token_rotation_is_never_replayed,
    covers = ["C-53"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let replies = vec![
            Reply::ok(&fx.tokens("ghr_ONE", 100)),
            Reply::ok(&fx.tokens("ghr_TWO", 100)),
        ];
        let token_path = "/login/oauth/access_token";
        g.gh.reroute_when("POST", token_path, "refresh_token", replies);
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", Some(("ghu_NEARLY_EXPIRED", now() + 100)));
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.mint_status(), "ok");
        let bodies: Vec<Value> =
            g.gh.requests_to("POST", token_path)
                .iter()
                .map(Request::json)
                .collect();
        assert_eq!(bodies.len(), 2, "access within the margin is refreshed");
        assert_eq!(bodies[0]["refresh_token"], "ghr_OLD");
        assert_eq!(
            bodies[1]["refresh_token"], "ghr_ONE",
            "rotated token used next"
        );
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_TWO");
    }
);

parity!(
    app_you_concurrent_mints_share_one_rotation,
    covers = ["C-53"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let replies = vec![
            Reply::ok(&fx.tokens("ghr_ONE", 28800)),
            Reply::oauth_error("bad_refresh_token"),
        ];
        let token_path = "/login/oauth/access_token";
        g.gh.reroute_when("POST", token_path, "refresh_token", replies);
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        g.gh.clear_requests();

        let statuses: Vec<String> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..2).map(|_| s.spawn(|| g.mint_status())).collect();
            jobs.into_iter()
                .map(|j| j.join().expect("env thread"))
                .collect()
        });
        assert_eq!(statuses, ["ok", "ok"]);
        assert_eq!(count(&g.gh.requests(), "POST", token_path), 1);
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_ONE");
    }
);

parity!(
    app_you_login_problems_fail_without_gh_fallback,
    covers = ["C-53", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        let token_path = "/login/oauth/access_token";
        let scoped_path = format!("/applications/{}/token/scoped", g.fx.client_id);
        g.setup();
        g.identity("you");
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "failed", "not logged in");
        assert_eq!(count(&g.gh.requests(), "POST", token_path), 0);

        g.seed_user("ghr_DEAD", None);
        let dead = Reply::oauth_error("bad_refresh_token");
        g.gh.reroute_when("POST", token_path, "refresh_token", vec![dead]);
        g.gh.clear_requests();
        let rejected = g.mint();
        assert_eq!(rejected["status"], "failed");
        assert!(
            rejected["error"].as_str().unwrap().contains("login"),
            "{rejected}"
        );
        assert_eq!(count(&g.gh.requests(), "POST", &scoped_path), 0);
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_DEAD");

        let other = Reply::oauth_error("incorrect_client_credentials");
        g.gh.reroute_when("POST", token_path, "refresh_token", vec![other]);
        let failed = g.mint();
        assert_eq!(failed["status"], "failed");
        assert!(failed["error"]
            .as_str()
            .unwrap()
            .contains("incorrect_client_credentials"));
        g.no_gh_fallback();
        assert_secret_absent(&g.w, "ghp_HOSTTOKEN", &[]);
    }
);

parity!(
    bot_mint_scopes_to_one_repo_id_without_caching,
    covers = ["C-55"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let client_secret = g.setup();
        g.identity("bot");
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        let reqs = g.gh.requests();
        let tokens_path = format!("/app/installations/{}/access_tokens", fx.installation_id);
        let want = [
            "GET /repos/acme/widget/installation".to_string(),
            format!("POST {tokens_path}"),
            "GET /repos/acme/widget".to_string(),
            format!("POST {tokens_path}"),
        ];
        assert_subseq(&reqs, &want.iter().map(String::as_str).collect::<Vec<_>>());

        let posts = g.gh.requests_to("POST", &tokens_path);
        assert_eq!(posts.len(), 2);
        assert!(
            posts.iter().all(|p| p.jwt().is_some()),
            "installation tokens use the App JWT"
        );
        assert_eq!(
            posts[0].json(),
            json!({ "permissions": { "metadata": "read" } })
        );
        assert_eq!(
            posts[1].json(),
            json!({ "repository_ids": [fx.repo_id], "permissions": permissions_write() })
        );
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.probe_token.as_str()));

        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(
            count(&g.gh.requests(), "POST", &tokens_path),
            2,
            "no caching"
        );
        for token in [&fx.bot_token, &fx.probe_token] {
            assert_secret_absent(&g.w, token, &[]);
        }
        assert_secret_absent(&g.w, &client_secret, &["github/"]);
    }
);

parity!(
    mint_uses_canonical_ids_across_rename_redirects,
    covers = ["C-57", "C-50"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let mut renamed = fx.clone();
        renamed.name = "renamed".into();
        let target = format!("{}{}", g.gh.url(), renamed.repo_path());
        g.gh.reroute("GET", &fx.repo_path(), vec![Reply::redirect(301, &target)]);
        g.gh.route(
            "GET",
            &renamed.repo_path(),
            vec![Reply::ok(&renamed.repo_json())],
        );
        g.setup();
        g.seed_user("ghr_OLD", None);

        for identity in ["bot", "you"] {
            g.identity(identity);
            g.gh.clear_requests();
            assert_eq!(g.mint_status(), "ok", "{identity}");
            let reqs = g.gh.requests();
            let first = &g.gh.requests_to("GET", &fx.repo_path())[0];
            let second = &g.gh.requests_to("GET", &renamed.repo_path())[0];
            assert!(first.header("authorization").is_some());
            assert_eq!(
                first.header("authorization"),
                second.header("authorization"),
                "{identity}: auth is kept on the same-host redirect"
            );
            let final_post = reqs
                .iter()
                .rfind(|r| {
                    r.method == "POST"
                        && (r.path.contains("access_tokens") || r.path.contains("scoped"))
                })
                .expect("token request");
            let body = final_post.json();
            assert_eq!(body["repository_ids"], json!([fx.repo_id]), "{identity}");
            for name in ["widget", "renamed"] {
                assert!(
                    !final_post.body.contains(name),
                    "{identity}: {name} in {}",
                    final_post.body
                );
            }
        }
    }
);

parity!(
    mint_install_errors_are_actionable_and_others_pass_through,
    covers = ["C-58", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.setup();
        g.identity("bot");
        g.installation(404);

        let from_file = g.mint();
        assert_eq!(from_file["status"], "failed");
        assert!(
            from_file["error"].as_str().unwrap().contains("moor-bot"),
            "{from_file}"
        );

        std::fs::remove_file(g.w.state_dir().join("github/app.slug")).expect("rm slug");
        g.gh.clear_requests();
        assert!(g.mint_error().contains("moor-bot"), "slug from GET /app");
        assert!(count(&g.gh.requests(), "GET", "/app") >= 1);

        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let generic = g.mint();
        assert_eq!(generic["status"], "failed");
        assert!(!generic["error"].as_str().unwrap().contains("moor-bot"));

        g.installation(500);
        let other = g.mint_error();
        assert!(other.contains("500"), "{other}");
        assert!(!other.contains("installations/new"));

        g.gh.reroute(
            "GET",
            "/app",
            vec![Reply::ok(&json!({ "slug": "moor-bot" }))],
        );
        g.installation(200);
        let tokens = format!("/app/installations/{}/access_tokens", g.fx.installation_id);
        let denied = "App does not have access to the given target";
        let forbidden = Reply::message(403, denied);
        g.gh.reroute_when("POST", &tokens, "repository_ids", vec![forbidden]);
        let target = g.mint();
        assert_eq!(target["status"], "failed");
        assert!(
            target["error"].as_str().unwrap().contains("moor-bot"),
            "{target}"
        );
    }
);

parity!(
    app_you_scoped_403_target_error_names_install_page,
    covers = ["C-58"],
    tags = ["new_only(ScopedInstallErrorMapped)"],
    {
        let g = app_you_world();
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        let scoped = format!("/applications/{}/token/scoped", g.fx.client_id);
        let denied = "App does not have access to the given target";
        g.gh.reroute("POST", &scoped, vec![Reply::message(403, denied)]);
        let target = g.mint();
        assert_eq!(target["status"], "failed");
        assert!(
            target["error"].as_str().unwrap().contains("moor-bot"),
            "{target}"
        );
    }
);

fn probe_case(g: &Gx, identity: &str, installation: u16) -> Value {
    g.identity(identity);
    g.installation(installation);
    g.gh.clear_requests();
    g.resolved()
}

parity!(
    resolved_identity_modes_and_app_installed_probe,
    covers = ["C-35", "C-59", "C-60"],
    tags = [],
    {
        let g = app_you_world();
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );

        g.identity("auto");
        let no_app = g.resolved();
        assert_eq!(no_app["mode"], "gh-you");
        assert_eq!(no_app["app_installed"], "n/a");
        assert!(no_app["token_source"].as_str().unwrap().contains("gh"));
        assert!(
            g.gh.requests().is_empty(),
            "auto without an App makes no HTTP call"
        );

        g.identity("bot");
        let bot_no_app = g.resolved();
        assert_eq!(
            (&bot_no_app["mode"], &bot_no_app["app_installed"]),
            (&json!("bot"), &json!("n/a"))
        );

        g.setup();
        let auto_yes = probe_case(&g, "auto", 200);
        assert_eq!(
            (&auto_yes["mode"], &auto_yes["app_installed"]),
            (&json!("app-you"), &json!("yes"))
        );
        let probes = g.gh.requests_to("GET", "/repos/acme/widget/installation");
        assert!(!probes.is_empty() && probes.iter().all(|p| p.jwt().is_some()));

        let auto_no = probe_case(&g, "auto", 404);
        assert_eq!(
            (&auto_no["mode"], &auto_no["app_installed"]),
            (&json!("gh-you"), &json!("no"))
        );

        let gh_calls = g.gh_token_calls();
        let broken = probe_case(&g, "auto", 500);
        assert!(broken["mode"].is_null(), "{broken}");
        assert_eq!(broken["app_installed"], "unknown");
        let err = broken["error"].as_str().expect("resolution error");
        assert!(err.contains("acme/widget") && err.contains("500"), "{err}");
        assert_eq!(
            g.gh_token_calls(),
            gh_calls,
            "a failed probe must not fall back to gh"
        );

        for (identity, mode) in [("bot", "bot"), ("you", "app-you")] {
            for (status, installed) in [(200, "yes"), (404, "no"), (500, "unknown")] {
                let r = probe_case(&g, identity, status);
                assert_eq!(r["mode"], mode, "{identity} {status}");
                assert_eq!(r["app_installed"], installed, "{identity} {status}");
            }
        }
        let sources: Vec<String> = ["bot", "you", "auto"]
            .iter()
            .map(|i| {
                let status = if *i == "auto" { 404 } else { 200 };
                probe_case(&g, i, status)["token_source"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(sources[0] != sources[1] && sources[1] != sources[2] && sources[0] != sources[2]);

        g.user(json!({ "github": { "identity": "auto", "you_token_cmd": "echo custom-cmd" } }));
        g.installation(404);
        let custom = g.resolved();
        assert!(custom["token_source"]
            .as_str()
            .unwrap()
            .contains("echo custom-cmd"));
    }
);

parity!(
    resolved_identity_unknown_for_non_github_origin,
    covers = ["C-35"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.setup();
        g.identity("auto");
        let r = g.resolved();
        assert!(r["mode"].is_null(), "{r}");
        assert_eq!(r["app_installed"], "unknown");
        assert!(r["token_source"].is_null());
        assert!(!r["error"].as_str().unwrap_or("").is_empty());
    }
);

parity!(
    mint_check_skipped_failed_and_exit_stays_ok,
    covers = ["C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );
        g.identity("auto");
        let none = g.mint();
        assert_eq!(none["status"], "skipped");
        assert!(
            g.gh.requests().is_empty() && g.gh_token_calls() == 0,
            "no App: nothing minted"
        );

        g.setup();
        g.identity("bot");
        g.git(&["remote", "remove", "origin"]);
        assert_eq!(g.mint_status(), "skipped", "no origin remote");
        g.git(&["remote", "add", "origin", "https://example.com/acme/widget"]);
        assert_eq!(g.mint_status(), "skipped", "origin is not github.com");
        g.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/widget",
        ]);
        assert_eq!(g.mint_status(), "ok");

        let tokens_path = format!("/app/installations/{}/access_tokens", g.fx.installation_id);
        g.gh.reroute_when(
            "POST",
            &tokens_path,
            "repository_ids",
            vec![Reply::message(500, "boom")],
        );
        let failed = g.mint();
        assert_eq!(failed["status"], "failed");
        assert!(
            failed["error"].as_str().unwrap().contains("500"),
            "{failed}"
        );
    }
);

parity!(
    gh_you_mint_runs_command_per_mint_and_checks_push,
    covers = ["C-61", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.setup();
        g.installation(404);
        g.identity("auto");
        let token = g.secret("ghyou");
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout(format!("{token}\n"))],
        );
        g.seed_user("ghr_DEAD", None);
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "refresh_token",
            vec![Reply::oauth_error("bad_refresh_token")],
        );
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok", "gh-you ignores an expired App login");
        assert_eq!(g.gh_token_calls(), 1);
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.gh_token_calls(), 2, "the command runs on every mint");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(
            repo_req.header("authorization"),
            Some(format!("token {token}").as_str())
        );
        assert_eq!(
            count(&g.gh.requests(), "POST", "/login/oauth/access_token"),
            0
        );

        let mut cannot = g.fx.clone();
        cannot.can_push = false;
        g.gh.reroute(
            "GET",
            &cannot.repo_path(),
            vec![Reply::ok(&cannot.repo_json())],
        );
        assert_eq!(g.mint_status(), "failed", "push permission is required");

        g.gh.reroute("GET", &g.fx.repo_path(), vec![Reply::ok(&g.fx.repo_json())]);
        g.w.prepend_rules("gh", [Rule::on(&["auth", "token"]).exit(1)]);
        assert_eq!(g.mint_status(), "failed", "failing command");
        g.w.prepend_rules("gh", [Rule::on(&["auth", "token"]).stdout("\n")]);
        assert_eq!(g.mint_status(), "failed", "empty output");

        let custom_file = g.w.root().join("custom-token");
        let custom = g.secret("custom");
        write_file(&custom_file, &custom, 0o600);
        let cmd = format!("cat '{}'", custom_file.display());
        g.user(json!({ "github": { "identity": "auto", "you_token_cmd": cmd } }));
        let calls = g.gh_token_calls();
        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.gh_token_calls(), calls, "custom command replaces gh");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(
            repo_req.header("authorization"),
            Some(format!("token {custom}").as_str())
        );
        for s in [token.as_str(), custom.as_str()] {
            assert_secret_absent(&g.w, s, &[]);
        }
    }
);

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

fn box_start(g: &Gx, name: &str) -> Outcome<parity::adapter::TaskView> {
    write_file(
        &g.repo.path.join(".moor.yaml"),
        &json!({ "agent": { "location": "box" } }).to_string(),
        0o644,
    );
    let opts = StartOpts {
        goal: Some("go".into()),
        location: Some("box".into()),
        ..Default::default()
    };
    moor().start(&g.w, &g.repo.path, name, &opts)
}

fn identity_events(g: &Gx, name: &str) -> Vec<Value> {
    moor()
        .events(&g.w, name)
        .into_iter()
        .filter(|e| e.kind == EventKind::Other("github_identity".into()))
        .map(|e| e.payload)
        .collect()
}

parity!(
    box_start_preflight_mint_failure_acquires_no_box,
    covers = ["C-56"],
    tags = [],
    {
        let g = app_you_world();
        flow_fakes(&g.w);
        g.identity("bot");
        let name = g.w.task_name("nopreflight");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stderr);
        let kinds: Vec<EventKind> = moor()
            .events(&g.w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::CredentialFailed), "{kinds:?}");
        assert_never_called(&g.w.all_calls(), "crabbox", &["warmup"]);
        assert_never_called(&g.w.all_calls(), "herdr", &["pane", "run"]);
        assert!(g.gh.requests().is_empty(), "no App, no HTTP");
        g.no_gh_fallback();
    }
);

parity!(
    proxy_records_github_identity_mode_only,
    covers = ["C-63", "C-60"],
    tags = [],
    {
        let g = app_you_world();
        flow_fakes(&g.w);
        g.setup();
        g.identity("bot");
        let name = g.w.task_name("ident");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        let events = identity_events(&g, &name);
        assert_eq!(events, [json!({ "mode": "bot" })], "one event, mode only");
        let closed = moor().close(&g.w, &g.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&g.w);
        for token in [&g.fx.bot_token, &g.fx.probe_token] {
            assert_secret_absent(&g.w, token, &[]);
        }
    }
);

parity!(
    proxy_records_no_identity_without_github_repo,
    covers = ["C-63"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        flow_fakes(&g.w);
        let name = g.w.task_name("nogh");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        assert!(identity_events(&g, &name).is_empty());
        let closed = moor().close(&g.w, &g.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&g.w);
    }
);
