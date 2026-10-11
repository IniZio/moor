use parity::parity;

use super::support::*;
use super::upstream_support::*;

parity!(
    proxy_git_route_is_scoped_to_the_task_repo_and_git_endpoints,
    covers = ["C-70", "C-71"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        let ep = f.up();
        let sup = cmdline(f.pid(&f.name));
        assert!(sup
            .windows(2)
            .any(|p| p[0] == "--repo" && p[1] == "acme/widget"));
        let allowed = [
            (
                "GET",
                "/github/acme/widget/info/refs?service=git-upload-pack",
            ),
            ("HEAD", "/github/acme/widget/info/refs"),
            ("POST", "/github/acme/widget.git/git-upload-pack"),
            ("POST", "/github/ACME/Widget/git-receive-pack"),
        ];
        expect_all(&ep, 200, &allowed);
        assert_eq!(up.seen.lock().unwrap().len(), allowed.len());
        let denied = [
            ("GET", "/github/acme/other/info/refs"),
            ("GET", "/github/other/widget/info/refs"),
            ("GET", "/github/acme/widget/archive/main.tar.gz"),
            ("GET", "/github/acme/widget/info/refs/x"),
            ("GET", "/github/acme/widget/../other/info/refs"),
            ("GET", "/github/acme/widget/%2e%2e/info/refs"),
            ("GET", "/github/acme/widget.git.git/info/refs"),
            ("POST", "/github/acme/widget/info/refs"),
            ("GET", "/github/acme/widget/git-upload-pack"),
            ("GET", "/github/acme/widget/git-receive-pack"),
            ("PUT", "/github/acme/widget/git-receive-pack"),
            ("GET", "/github/api/v3/repos/acme/widget"),
            ("GET", "/github/"),
            ("GET", "/github"),
        ];
        expect_all(&ep, 403, &denied);
        assert_eq!(
            up.seen.lock().unwrap().len(),
            allowed.len(),
            "denied reached upstream"
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_rest_route_allows_only_pull_endpoints_of_the_task_repo,
    covers = ["C-73"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        let ep = f.up();
        let r = "/github-api/repos/acme/widget";
        let allowed = [
            ("GET", format!("{r}/pulls?state=open")),
            ("POST", format!("{r}/pulls")),
            ("GET", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/pulls/12")),
            ("GET", "/github-api/repos/ACME/Widget/pulls".to_string()),
        ];
        for (m, p) in &allowed {
            let got = via(&ep, m, p);
            assert_eq!(got.status(), Some(200), "{m} {p}: {}", got.raw);
        }
        let hits = up.seen.lock().unwrap().len();
        let denied = [
            ("GET", "/github-api/repos/acme/other/pulls".to_string()),
            ("GET", "/github-api/repos/other/widget/pulls".to_string()),
            ("GET", "/github-api/user".to_string()),
            ("GET", r.to_string()),
            ("GET", format!("{r}/contents/README.md")),
            ("PUT", format!("{r}/pulls/12/merge")),
            ("GET", format!("{r}/pulls/12/files")),
            ("GET", format!("{r}/pulls/abc")),
            ("DELETE", format!("{r}/pulls/12")),
            ("PUT", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/pulls")),
            ("POST", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/issues/12/comments")),
            ("GET", format!("{r}/issues/12")),
            ("GET", format!("{r}/pulls/../issues/12")),
            ("GET", format!("{r}/pulls/./12")),
            ("GET", format!("{r}/pulls/%31")),
            ("GET", format!("{r}/pulls/1\\2")),
            ("GET", format!("{r}/pulls//12")),
            ("GET", format!("{r}/pulls/")),
        ];
        for (m, p) in &denied {
            let got = via(&ep, m, p);
            assert_eq!(got.status(), Some(403), "{m} {p}: {}", got.raw);
            assert_eq!(got.body(), "forbidden\n");
        }
        assert_eq!(
            up.seen.lock().unwrap().len(),
            hits,
            "denied reached upstream"
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_mint_failure_is_502_and_closes_the_connection,
    covers = ["C-72"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        user_with_token_cmd(&f, "false");
        let ep = f.up();
        let r = via(&ep, "GET", "/github/acme/widget/info/refs");
        assert_eq!(r.status(), Some(502), "{}", r.raw);
        assert_eq!(r.body(), "credential unavailable\n");
        assert!(r.head_has("connection: close"));
        assert!(up.seen.lock().unwrap().is_empty());
        f.close(&f.name);
    }
);
