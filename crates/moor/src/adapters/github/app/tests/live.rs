//! The App against real GitHub. Run by hand with real credentials:
//! `MOOR_TEST_APP_ID=.. MOOR_TEST_APP_KEY=/path/key.pem MOOR_TEST_APP_PULL=owner/name#N
//! MOOR_TEST_APP_LOGIN=.. cargo test -p moor --lib app::tests::live -- --ignored --nocapture`
#![allow(
    clippy::print_stdout,
    reason = "a check run by hand prints what it read"
)]

use moor_core::config::GitHubAppConfig;
use moor_core::seam::clock::Clock as _;
use moor_core::seam::forge::ForgeRepo;

use crate::adapters::clock::SystemClock;
use crate::adapters::github::app::AppClient;

fn var(name: &str) -> String {
    std::env::var(name).expect(name)
}

#[test]
#[ignore = "needs MOOR_TEST_APP_ID and a real App key"]
fn a_real_app_reads_a_pull_request_and_its_author_permission() {
    let config = GitHubAppConfig {
        id: var("MOOR_TEST_APP_ID").parse().unwrap(),
        private_key_file: var("MOOR_TEST_APP_KEY").into(),
        webhook_secret_file: None,
    };
    let app = AppClient::load(&config, std::path::Path::to_path_buf).unwrap();
    let now = SystemClock::real().now();
    let pull = var("MOOR_TEST_APP_PULL");
    let (repo, number) = pull.split_once('#').unwrap();
    let repo = ForgeRepo::parse(repo).unwrap();
    let installation = app
        .installation_of(&repo, now)
        .unwrap()
        .expect("App not installed");
    let token = app.token(installation, now).unwrap();
    let read = app.pull(&repo, number.parse().unwrap(), &token).unwrap();
    let permission = app
        .permission(&repo, &var("MOOR_TEST_APP_LOGIN"), &token)
        .unwrap();
    let installed = app.installed(now).unwrap();
    println!(
        "pull {read:?}\npermission {permission:?}\ninstall {}",
        installed.install_url
    );
    assert!(installed.repos.contains(&repo), "{:?}", installed.repos);
}
