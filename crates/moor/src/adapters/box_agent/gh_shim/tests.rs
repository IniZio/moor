//! Tests that run the real shim script against a fake HTTP server.

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::{gh_shim, GH_SHIM};

type Res = Result<(), Box<dyn Error>>;

/// One request as the fake server saw it.
struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
    raw: String,
}

/// A canned reply: method, path with query, status, body.
type Route = (&'static str, &'static str, u16, &'static str);

/// Port, stop flag and recorded requests of a fake server.
type Served = (u16, Arc<AtomicBool>, Arc<Mutex<Vec<Req>>>);

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    reqs: Vec<Req>,
}

fn read_req(stream: &std::net::TcpStream) -> Result<Req, Box<dyn Error>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut raw = line.clone();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h)?;
        raw.push_str(&h);
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_lowercase(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map_or(0, |(_, v)| v.parse().unwrap_or(0));
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    let body = String::from_utf8(buf)?;
    raw.push_str(&body);
    Ok(Req {
        method,
        path,
        headers,
        body,
        raw,
    })
}

fn serve(routes: &[Route]) -> Result<Served, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let routes = routes.to_vec();
    let (stop2, seen2) = (stop.clone(), seen.clone());
    thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            let Ok((mut s, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            let Ok(req) = read_req(&s) else { continue };
            let (status, body) = routes
                .iter()
                .find(|(m, p, _, _)| *m == req.method && *p == req.path)
                .map_or((404, r#"{"message":"Not Found"}"#), |r| (r.2, r.3));
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
            if let Ok(mut v) = seen2.lock() {
                v.push(req);
            }
        }
    });
    Ok((port, stop, seen))
}

struct Fx {
    dir: tempfile::TempDir,
}

impl Fx {
    fn new() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("bin"))?;
        std::fs::write(
            dir.path().join("bin/git"),
            "#!/bin/sh\nif [ \"$1 $2 $3\" = \"rev-parse --abbrev-ref HEAD\" ]; then echo feat/x; else exit 1; fi\n",
        )?;
        std::fs::set_permissions(
            dir.path().join("bin/git"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )?;
        std::fs::write(dir.path().join("gh"), GH_SHIM)?;
        Ok(Self { dir })
    }

    fn file(&self, name: &str, content: &str) -> Result<String, Box<dyn Error>> {
        let p: PathBuf = self.dir.path().join(name);
        std::fs::write(&p, content)?;
        Ok(p.to_string_lossy().into_owned())
    }

    fn run(&self, args: &[&str], routes: &[Route]) -> Result<Out, Box<dyn Error>> {
        let (port, stop, seen) = serve(routes)?;
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = Command::new("sh")
            .arg(self.dir.path().join("gh"))
            .args(args)
            .current_dir(self.dir.path())
            .env("PATH", path)
            .env(
                "MOOR_GH_API",
                format!("http://127.0.0.1:{port}/n0nce/forge"),
            )
            .env("MOOR_GH_REPO", "Acme/Widgets")
            .env("GH_TOKEN", "leak-me")
            .env("GITHUB_TOKEN", "leak-me")
            .output()?;
        stop.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(20));
        let reqs = std::mem::take(&mut *seen.lock().map_err(|_| "poisoned")?);
        Ok(Out {
            code: out.status.code(),
            stdout: String::from_utf8(out.stdout)?,
            stderr: String::from_utf8(out.stderr)?,
            reqs,
        })
    }
}

const REPO: &str = "/n0nce/forge/repos/Acme/Widgets";
const REPO_GET: Route = (
    "GET",
    "/n0nce/forge/repos/Acme/Widgets",
    200,
    r#"{"default_branch":"main","name":"Widgets"}"#,
);
const PULLS: &str = "/n0nce/forge/repos/Acme/Widgets/pulls";
const CREATED: &str = r#"{"html_url":"https://github.com/Acme/Widgets/pull/7","number":7}"#;

#[test]
fn gh_shim_install_helper_is_executable() {
    let (bytes, mode) = gh_shim();
    assert_eq!(bytes, GH_SHIM.as_bytes());
    assert_eq!(mode, 0o755);
}

#[test]
fn gh_shim_never_mentions_token_variables() {
    assert!(!GH_SHIM.contains("GH_TOKEN"));
    assert!(!GH_SHIM.contains("GITHUB_TOKEN"));
}

#[test]
fn gh_shim_rejects_unsupported_commands() -> Res {
    let fx = Fx::new()?;
    for args in [&["issue", "list"][..], &["pr", "merge", "1"], &[]] {
        let o = fx.run(args, &[])?;
        assert_eq!(o.code, Some(2), "{args:?}");
        assert_eq!(o.stderr.trim().lines().count(), 1, "{args:?}");
        assert!(o.stderr.contains("gh pr create, gh pr view, gh pr list"));
        assert!(o.reqs.is_empty());
    }
    Ok(())
}

#[test]
fn gh_shim_repo_flag_is_checked_case_insensitively() -> Res {
    let fx = Fx::new()?;
    let ok = fx.run(
        &["pr", "view", "7", "--json", "title", "-R", "acme/widgets"],
        &[("GET", "/n0nce/forge/repos/Acme/Widgets/pulls/7", 200, "{}")],
    )?;
    assert_eq!(ok.code, Some(0), "{}", ok.stderr);
    for flag in [
        &["-R", "other/repo"][..],
        &["--repo", "other/repo"],
        &["--repo=other/repo"],
    ] {
        let mut args = vec!["pr", "list"];
        args.extend_from_slice(flag);
        let o = fx.run(&args, &[])?;
        assert_eq!(o.code, Some(2), "{flag:?}");
        assert!(o.stderr.contains("moor only reaches"));
        assert!(o.reqs.is_empty());
    }
    let o = fx.run(
        &["pr", "list", "--repo=ACME/widgets"],
        &[(
            "GET",
            "/n0nce/forge/repos/Acme/Widgets/pulls?state=open&per_page=30",
            200,
            "[]",
        )],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    Ok(())
}

#[test]
fn gh_shim_rejects_foreign_head_owner() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(
        &["pr", "create", "--title", "T", "--head", "evil:feat"],
        &[REPO_GET],
    )?;
    assert_eq!(o.code, Some(2));
    assert!(o.reqs.is_empty());
    Ok(())
}

#[test]
fn gh_shim_rejects_unknown_flag() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(&["pr", "create", "--bogus"], &[])?;
    assert_eq!(o.code, Some(2));
    assert!(o.stderr.contains("gh: unsupported flag:"));
    assert!(o.reqs.is_empty());
    Ok(())
}

#[test]
fn gh_shim_rejects_values_that_would_leave_the_endpoint() -> Res {
    let fx = Fx::new()?;
    for args in [
        &["pr", "view", "../x"][..],
        &["pr", "list", "-L", "5x"],
        &["pr", "list", "--state", "merged"],
        &["pr", "list", "--head", "a&state=closed"],
    ] {
        let o = fx.run(args, &[])?;
        assert_eq!(o.code, Some(2), "{args:?}");
        assert!(o.reqs.is_empty(), "{args:?}");
    }
    Ok(())
}

#[test]
fn gh_shim_create_resolves_default_base_and_posts_exact_json() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(
        &["pr", "create", "--title", "T"],
        &[REPO_GET, ("POST", PULLS, 201, CREATED)],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert!(o.stdout.contains("https://github.com/Acme/Widgets/pull/7"));
    let [get, post] = &o.reqs[..] else {
        return Err("want 2 requests".into());
    };
    assert_eq!((get.method.as_str(), get.path.as_str()), ("GET", REPO));
    assert_eq!(post.method, "POST");
    assert_eq!(
        post.body,
        r#"{"title":"T","body":"","head":"feat/x","base":"main","draft":false}"#
    );
    Ok(())
}

#[test]
fn gh_shim_create_escapes_json_strings() -> Res {
    let fx = Fx::new()?;
    let body = fx.file("body.md", "line one\n\tindented")?;
    let o = fx.run(
        &[
            "pr",
            "create",
            "--title",
            r#"say "hi" \ now"#,
            "--body-file",
            &body,
        ],
        &[REPO_GET, ("POST", PULLS, 201, CREATED)],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let post = o.reqs.last().ok_or("no request")?;
    assert!(
        post.body.contains(r#""title":"say \"hi\" \\ now""#),
        "{}",
        post.body
    );
    assert!(
        post.body.contains(r#""body":"line one\n\tindented""#),
        "{}",
        post.body
    );
    Ok(())
}

#[test]
fn gh_shim_create_with_explicit_base_head_and_draft_skips_lookup() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(
        &[
            "pr", "create", "--title", "T", "--base", "dev", "--head", "feat/y", "--draft",
        ],
        &[("POST", PULLS, 201, CREATED)],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let [post] = &o.reqs[..] else {
        return Err("want 1 request".into());
    };
    assert_eq!(post.method, "POST");
    assert!(post.body.contains(r#""base":"dev""#));
    assert!(post.body.contains(r#""head":"feat/y""#));
    assert!(post.body.contains(r#""draft":true"#));
    Ok(())
}

#[test]
fn gh_shim_sends_no_credentials_and_asks_for_github_json() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(
        &["pr", "create", "--title", "T"],
        &[REPO_GET, ("POST", PULLS, 201, CREATED)],
    )?;
    assert_eq!(o.reqs.len(), 2);
    for r in &o.reqs {
        assert!(r.headers.iter().all(|(k, _)| k != "authorization"));
        assert!(!r.raw.contains("leak-me"));
        assert!(r.headers.contains(&(
            "accept".to_owned(),
            "application/vnd.github+json".to_owned()
        )));
    }
    Ok(())
}

#[test]
fn gh_shim_reports_http_failure_status_and_message() -> Res {
    let fx = Fx::new()?;
    let o = fx.run(
        &["pr", "create", "--title", "T"],
        &[
            REPO_GET,
            ("POST", PULLS, 422, r#"{"message":"Validation Failed"}"#),
        ],
    )?;
    assert_eq!(o.code, Some(1));
    assert!(o.stderr.contains("422"), "{}", o.stderr);
    assert!(o.stderr.contains("Validation Failed"), "{}", o.stderr);
    Ok(())
}

#[test]
fn gh_shim_view_json_passes_body_through() -> Res {
    let fx = Fx::new()?;
    let body = r#"{"title":"T","html_url":"u","state":"open"}"#;
    let o = fx.run(
        &["pr", "view", "7", "--json", "title,url"],
        &[("GET", "/n0nce/forge/repos/Acme/Widgets/pulls/7", 200, body)],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(o.stdout.trim_end(), body);
    Ok(())
}

#[test]
fn gh_shim_view_plain_prints_title_state_url() -> Res {
    let fx = Fx::new()?;
    let body =
        r#"{"title":"Fix it","state":"open","html_url":"https://github.com/Acme/Widgets/pull/7"}"#;
    let o = fx.run(
        &["pr", "view", "7"],
        &[("GET", "/n0nce/forge/repos/Acme/Widgets/pulls/7", 200, body)],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert!(lines.contains(&"title:\tFix it"), "{lines:?}");
    assert!(lines.contains(&"state:\topen"), "{lines:?}");
    assert!(
        lines.contains(&"url:\thttps://github.com/Acme/Widgets/pull/7"),
        "{lines:?}"
    );
    Ok(())
}

#[test]
fn gh_shim_list_prints_pull_urls_in_order() -> Res {
    let fx = Fx::new()?;
    let body = r#"[{"html_url":"https://github.com/Acme/Widgets/pull/9","head":{"repo":{"html_url":"https://github.com/Acme/Widgets"}}},{"html_url":"https://github.com/Acme/Widgets/pull/3","base":{"repo":{"html_url":"https://github.com/Acme/Other"}}}]"#;
    let o = fx.run(
        &["pr", "list", "--state", "all", "-L", "5"],
        &[(
            "GET",
            "/n0nce/forge/repos/Acme/Widgets/pulls?state=all&per_page=5",
            200,
            body,
        )],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(
        o.stdout,
        "https://github.com/Acme/Widgets/pull/9\nhttps://github.com/Acme/Widgets/pull/3\n"
    );
    Ok(())
}

#[test]
fn gh_shim_list_json_passes_body_through() -> Res {
    let fx = Fx::new()?;
    let body = r#"[{"number":1}]"#;
    let o = fx.run(
        &["pr", "list", "--json", "number"],
        &[(
            "GET",
            "/n0nce/forge/repos/Acme/Widgets/pulls?state=open&per_page=30",
            200,
            body,
        )],
    )?;
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(o.stdout.trim_end(), body);
    Ok(())
}
