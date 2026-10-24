//! The Docker proxy line run under a real `sh` in a temp `HOME`, with `jq` on the `PATH` or not.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::super::docker::line;
use super::super::egress::bridge_line;

const PORT: u16 = 41234;

const DEFAULT_ROUTE: &str = "172.17.0.0/16 src 172.17.0.1 linkdown\n";

/// A temp `HOME` and a `bin` directory that is the whole `PATH`.
struct DockerHome {
    home: PathBuf,
    _dir: tempfile::TempDir,
}

impl DockerHome {
    fn new(jq: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("moor-docker-")
            .tempdir()
            .unwrap();
        let home = dir.path().to_path_buf();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut tools = vec![
            "sh", "cmp", "mkdir", "mv", "rm", "cp", "chmod", "sed", "cat",
        ];
        if jq {
            tools.push("jq");
        }
        for tool in tools {
            let found = std::env::var("PATH")
                .unwrap()
                .split(':')
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists())
                .unwrap();
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        moor_testkit::write_executable(&bin.join("ip"), moor_testkit::FAKE_IP).unwrap();
        let home = Self { home, _dir: dir };
        home.bridge(Some(DEFAULT_ROUTE));
        home
    }

    fn bridge(&self, route: Option<&str>) {
        let file = self.home.join("docker0.route");
        match route {
            Some(r) => std::fs::write(file, r).unwrap(),
            None => std::fs::remove_file(file).unwrap(),
        }
    }

    fn file(&self) -> PathBuf {
        self.home.join(".docker/config.json")
    }

    fn put(&self, text: &str, mode: u32) {
        std::fs::create_dir_all(self.home.join(".docker")).unwrap();
        std::fs::write(self.file(), text).unwrap();
        std::fs::set_permissions(self.file(), std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn run_port(&self, port: u16) -> (bool, String) {
        let out = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("umask 022\n{}\n{}", bridge_line(), line(port)),
            ])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.home.join("bin"))
            .output()
            .unwrap();
        (out.status.success(), String::from_utf8(out.stderr).unwrap())
    }

    fn run(&self) -> (bool, String) {
        self.run_port(PORT)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.file()).unwrap()).unwrap()
    }

    fn mode(&self) -> u32 {
        std::fs::metadata(self.file()).unwrap().permissions().mode() & 0o777
    }

    fn inode(&self) -> u64 {
        std::fs::metadata(self.file()).unwrap().ino()
    }

    fn names(&self) -> Vec<String> {
        let mut n: Vec<String> = std::fs::read_dir(self.home.join(".docker"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    }
}

fn proxies() -> Value {
    proxies_at("172.17.0.1")
}

fn proxies_at(address: &str) -> Value {
    let url = format!("http://{address}:{PORT}");
    json!({"default": {
        "httpProxy": url,
        "httpsProxy": url,
        "noProxy": "localhost,127.0.0.1,::1,172.16.0.0/12"
    }})
}

#[test]
fn a_box_without_docker0_gets_no_docker_config() {
    let home = DockerHome::new(true);
    home.bridge(None);
    assert_eq!(home.run(), (true, String::new()));
    assert!(!home.home.join(".docker").exists());
}

#[test]
fn a_box_without_docker0_leaves_an_existing_docker_config_alone() {
    let home = DockerHome::new(true);
    home.run_port(PORT + 1);
    let before = std::fs::read_to_string(home.file()).unwrap();
    home.bridge(None);
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(std::fs::read_to_string(home.file()).unwrap(), before);
}

#[test]
fn the_config_names_the_address_docker0_has() {
    let home = DockerHome::new(true);
    home.bridge(Some(
        "10.200.0.0/24 proto kernel scope link src 10.200.0.1\n",
    ));
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(home.json(), json!({"proxies": proxies_at("10.200.0.1")}));
}

#[test]
fn docker0_that_appears_later_gets_the_config_on_the_next_run() {
    let home = DockerHome::new(true);
    home.bridge(None);
    home.run();
    home.bridge(Some(DEFAULT_ROUTE));
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(home.json(), json!({"proxies": proxies()}));
}

#[test]
fn a_box_with_no_docker_config_gets_one_naming_the_bridge_gateway_and_the_egress_port() {
    let home = DockerHome::new(false);
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(home.json(), json!({"proxies": proxies()}));
    assert_eq!(home.mode(), 0o600);
    assert_eq!(home.names(), ["config.json"]);
}

#[test]
fn an_existing_docker_config_keeps_its_other_keys_and_its_mode() {
    let home = DockerHome::new(true);
    home.put(
        r#"{"auths":{"ghcr.io":{"auth":"c2VjcmV0"}},"credsStore":"pass","proxies":{"default":{"ftpProxy":"ftp://x:1","httpProxy":"http://old:1"},"tcp://other:2375":{"httpProxy":"http://o:3"}}}"#,
        0o600,
    );
    assert_eq!(home.run(), (true, String::new()));
    let url = format!("http://172.17.0.1:{PORT}");
    let want = json!({
        "default": {
            "ftpProxy": "ftp://x:1",
            "httpProxy": url,
            "httpsProxy": url,
            "noProxy": "localhost,127.0.0.1,::1,172.16.0.0/12"
        },
        "tcp://other:2375": {"httpProxy": "http://o:3"}
    });
    assert_eq!(
        home.json(),
        json!({"auths":{"ghcr.io":{"auth":"c2VjcmV0"}},"credsStore":"pass","proxies":want})
    );
    assert_eq!(home.mode(), 0o600);
    assert_eq!(home.names(), ["config.json"]);
}

#[test]
fn a_config_that_already_holds_the_settings_is_not_rewritten() {
    let home = DockerHome::new(true);
    home.run();
    let inode = home.inode();
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(home.inode(), inode);
    home.put(r#"{"proxies": {"default": {"httpProxy": "x"}}}"#, 0o644);
    home.run();
    let inode = home.inode();
    home.run();
    assert_eq!((home.inode(), home.mode()), (inode, 0o644));
}

#[test]
fn a_changed_egress_port_replaces_the_old_one() {
    let home = DockerHome::new(true);
    home.run_port(PORT + 1);
    assert_eq!(home.run(), (true, String::new()));
    assert_eq!(home.json(), json!({"proxies": proxies()}));
}

#[test]
fn an_existing_config_without_jq_is_kept_and_the_line_warns_without_failing() {
    let home = DockerHome::new(false);
    let text = r#"{"credsStore":"pass"}"#;
    home.put(text, 0o600);
    let (ok, err) = home.run();
    assert!(ok);
    assert!(err.contains("the Box has no jq"), "{err}");
    assert_eq!(std::fs::read_to_string(home.file()).unwrap(), text);
}

#[test]
fn a_config_that_is_not_a_json_object_is_kept_and_the_line_warns_without_failing() {
    for text in ["not json", "[1]"] {
        let home = DockerHome::new(true);
        home.put(text, 0o600);
        let (ok, err) = home.run();
        assert!(ok, "{text}");
        assert!(err.contains("not a JSON object"), "{text}: {err}");
        assert_eq!(std::fs::read_to_string(home.file()).unwrap(), text);
        assert_eq!(home.names(), ["config.json"]);
    }
}

#[test]
fn a_dangling_symlink_or_one_to_an_empty_file_is_kept() {
    for target in [None, Some("")] {
        let home = DockerHome::new(true);
        let real = home.home.join("real.json");
        if let Some(text) = target {
            std::fs::write(&real, text).unwrap();
        }
        std::fs::create_dir_all(home.home.join(".docker")).unwrap();
        std::os::unix::fs::symlink(&real, home.file()).unwrap();
        let (ok, err) = home.run();
        assert!(ok, "{target:?}");
        assert!(err.contains("symlink"), "{target:?}: {err}");
        assert!(std::fs::symlink_metadata(home.file())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).ok().as_deref(), target);
    }
}

#[test]
fn a_config_that_is_a_symlink_is_kept() {
    let home = DockerHome::new(true);
    home.put("{}", 0o600);
    std::fs::rename(home.file(), home.home.join("real.json")).unwrap();
    std::os::unix::fs::symlink(home.home.join("real.json"), home.file()).unwrap();
    let (ok, err) = home.run();
    assert!(ok);
    assert!(err.contains("symlink"), "{err}");
    assert_eq!(
        std::fs::read_to_string(home.home.join("real.json")).unwrap(),
        "{}"
    );
}
