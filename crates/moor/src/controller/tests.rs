use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::MoorYamlSource;

use super::{serve, start};
use crate::env::Env;

fn env(dir: &Path) -> Env {
    Env::from_pairs([
        ("MOOR_STATE_DIR", dir.to_str().expect("utf8")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ])
}

fn config(dir: &Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("controller.yaml");
    std::fs::write(&path, text).expect("write config");
    path
}

#[derive(Clone, Default)]
struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("lock").clone()).expect("utf8")
    }
}

#[test]
fn socket_is_owner_only() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let running = start(&env(tmp.path()), &cfg).expect("start");
    assert_eq!(running.socket, tmp.path().join("controller.sock"));
    let mode = std::fs::metadata(&running.socket)
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn second_instance_names_first() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let first = start(&env(tmp.path()), &cfg).expect("start");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &cfg, out.clone());
    assert_eq!(code, 0);
    let want = format!(
        "already running (pid {}) on {}",
        std::process::id(),
        first.socket.display()
    );
    assert!(out.text().contains(&want), "{}", out.text());
}

#[test]
fn unknown_key_is_usage_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "bogus_key: 1\n");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &cfg, out.clone());
    assert_eq!(code, 2);
    assert!(out.text().contains("bogus_key"), "{}", out.text());
}

#[test]
fn missing_config_names_path() {
    let tmp = tempfile::tempdir().expect("tmp");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &tmp.path().join("nope.yaml"), out.clone());
    assert_eq!(code, 2);
    assert!(out.text().contains("nope.yaml"), "{}", out.text());
}

#[test]
fn client_gets_a_reply_to_hello() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let running = start(&env(tmp.path()), &cfg).expect("start");
    let socket = running.socket.clone();
    let sink: super::Sink = Arc::new(std::sync::Mutex::new(Buf::default()));
    std::thread::spawn(move || running.accept_loop(&sink));
    let mut stream = UnixStream::connect(&socket).expect("connect");
    stream
        .write_all(b"{\"frame\":\"hello\",\"protocol\":1}\n")
        .expect("send");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("read");
    assert!(!line.is_empty());
}

#[test]
fn missing_repo_moor_yaml_names_key() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(
        tmp.path(),
        "repos:\n  app:\n    url: https://example.com/app.git\n    moor_yaml:\n      path: nope.yaml\n",
    );
    let stop = start(&env(tmp.path()), &cfg).err().expect("stop");
    assert_eq!(stop.code, 2);
    assert!(
        stop.message.contains("repos.app.moor_yaml"),
        "{}",
        stop.message
    );
    assert!(stop.message.contains("nope.yaml"), "{}", stop.message);
}

#[test]
fn relative_repo_moor_yaml_resolves_against_config_dir() {
    let tmp = tempfile::tempdir().expect("tmp");
    std::fs::write(tmp.path().join("app.yaml"), "{}\n").expect("write overlay");
    let cfg = config(
        tmp.path(),
        "repos:\n  app:\n    url: https://example.com/app.git\n    moor_yaml:\n      path: app.yaml\n",
    );
    let running = start(&env(tmp.path()), &cfg).expect("start");
    let entry = &running.env.repos()["app"];
    let want = tmp.path().join("app.yaml").to_string_lossy().into_owned();
    assert_eq!(entry.moor_yaml, Some(MoorYamlSource::Path(want)));
}

#[test]
fn slack_token_file_must_be_owner_only() {
    let tmp = tempfile::tempdir().expect("tmp");
    let app = tmp.path().join("app");
    let bot = tmp.path().join("bot");
    for (path, mode) in [(&app, 0o644), (&bot, 0o600)] {
        std::fs::write(path, "xtok\n").expect("write token");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }
    let cfg = config(
        tmp.path(),
        &format!(
            "slack:\n  app_token_file: {}\n  bot_token_file: {}\n  channels: [C1]\n  users: [U1]\n",
            app.display(),
            bot.display()
        ),
    );
    let stop = start(&env(tmp.path()), &cfg).err().expect("stop");
    assert_eq!(stop.code, 2);
    assert!(
        stop.message.contains("slack app token file"),
        "{}",
        stop.message
    );
    assert!(!stop.message.contains("xtok"));
}

mod artifacts;
mod run_output;
