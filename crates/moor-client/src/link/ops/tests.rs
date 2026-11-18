use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::{in_box, run, Ran};

/// ssh ends a call only once the remote side closes its output, so that is what must happen
/// while the job still runs.
#[test]
fn in_box_closes_its_output_while_a_job_its_body_started_runs_on() {
    let home = tempfile::tempdir().unwrap();
    let mut child = moor_testkit::ChildGuard::new(
        Command::new("bash")
            .args(["-c", &in_box(7, "sleep 5 >/dev/null 2>&1 & echo started")])
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(Duration::from_secs(3));
    let _ = child.wait();
    assert_eq!(out.as_deref(), Ok("started\n"));
    assert!(home.path().join("moor/7").is_dir());
}

#[test]
fn a_cache_carries_its_volumes_both_ways() {
    let cache = crate::link::lease::Cache {
        volumes: vec![crate::link::lease::CacheVolume {
            group: "0123456789abcdef".to_owned(),
            name: "cargo".to_owned(),
            key: "0123456789abcdef-cargo".to_owned(),
            paths: vec!["~/.cargo/registry".to_owned()],
            size_gb: 4,
        }],
        scope: Some("0123456789abcdef".to_owned()),
    };
    let json = serde_json::to_string(&cache).unwrap();
    assert!(json.contains(r#""sizeGB":4"#), "{json}");
    assert_eq!(
        serde_json::from_str::<crate::link::lease::Cache>(&json).unwrap(),
        cache
    );
}

/// Runs `script` through the wrapper as the machine agent does: `/bin/sh -c` in `home`, whose
/// account's login shell is `shell`.
async fn exec_as(home: &Path, shell: &str, script: &str) -> Ran {
    let mut carrier = tokio::process::Command::new("/bin/sh");
    carrier
        .current_dir(home)
        .env("HOME", home)
        .env("SHELL", shell)
        .arg("-c");
    run(carrier, script, Duration::from_secs(30)).await.unwrap()
}

/// The login shell can be zsh, which has no `PIPESTATUS`, so a command runs under bash rather than
/// under whatever the account's login shell is.
#[tokio::test]
async fn a_command_runs_under_bash_whatever_the_login_shell_is() {
    let home = tempfile::tempdir().unwrap();

    let ran = exec_as(
        home.path(),
        "/bin/sh",
        r#"true | false; echo "${PIPESTATUS[*]}""#,
    )
    .await;

    assert_eq!((ran.exit_code, ran.output.as_str()), (Some(0), "0 1\n"));
}

/// The login shell still sets the command's environment and PATH, as it did before.
#[tokio::test]
async fn a_command_keeps_the_login_shells_environment_and_path() {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    std::fs::write(tools.join("mytool"), "#!/bin/sh\necho from-mytool\n").unwrap();
    std::fs::set_permissions(
        tools.join("mytool"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "export LOGIN_SEEN=yes\nPATH=\"$HOME/tools:$PATH\"\nexport PATH\n",
    )
    .unwrap();

    let ran = exec_as(home.path(), "/bin/sh", r#"echo "$LOGIN_SEEN"; mytool"#).await;

    assert_eq!(ran.output, "yes\nfrom-mytool\n");
}

/// A machine with no bash on the login shell's PATH runs the command under sh.
#[tokio::test]
async fn a_command_runs_under_sh_when_the_path_has_no_bash() {
    let home = tempfile::tempdir().unwrap();
    let only = home.path().join("only");
    std::fs::create_dir(&only).unwrap();
    std::os::unix::fs::symlink("/bin/sh", only.join("sh")).unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "PATH=\"$HOME/only\"\nexport PATH\n",
    )
    .unwrap();

    let ran = exec_as(
        home.path(),
        "/bin/sh",
        "command -v bash >/dev/null || echo no-bash",
    )
    .await;

    assert_eq!((ran.exit_code, ran.output.as_str()), (Some(0), "no-bash\n"));
}
