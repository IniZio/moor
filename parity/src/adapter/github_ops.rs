use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;

use crate::world::World;

#[derive(Debug, Clone, Default)]
pub struct GithubSetupOpts {
    pub app_id: Option<String>,
    pub client_id: Option<String>,
    pub key: Option<PathBuf>,
    pub repo: Option<String>,
    pub json: bool,
    pub secret_env: Option<String>,
    pub secret_stdin: Option<String>,
    pub extra_args: Vec<String>,
}

/// Snapshot of `<state>/github/` taken after the call, whatever its exit.
#[derive(Debug, Clone, Default)]
pub struct GithubStore {
    pub dir_mode: Option<u32>,
    pub files: BTreeMap<String, u32>,
    pub app: Option<Value>,
    pub user: Option<Value>,
    pub slug: Option<String>,
    pub pem: Option<Vec<u8>>,
}

impl GithubStore {
    pub fn read(w: &World) -> GithubStore {
        let dir = w.state_dir().join("github");
        let mode = |p: &std::path::Path| {
            std::fs::metadata(p)
                .ok()
                .map(|m| m.permissions().mode() & 0o777)
        };
        let json = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        };
        let files = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let m = mode(&e.path())?;
                        Some((e.file_name().to_string_lossy().into_owned(), m))
                    })
                    .collect()
            })
            .unwrap_or_default();
        GithubStore {
            dir_mode: mode(&dir),
            files,
            app: json("app.json"),
            user: json("user.json"),
            slug: std::fs::read_to_string(dir.join("app.slug"))
                .ok()
                .map(|s| s.trim().to_string()),
            pem: std::fs::read(dir.join("app.pem")).ok(),
        }
    }

    pub fn exists(&self) -> bool {
        self.dir_mode.is_some()
    }

    pub fn file_mode(&self, name: &str) -> Option<u32> {
        self.files.get(name).copied()
    }

    pub fn leftover_tmp(&self) -> Vec<&str> {
        self.files
            .keys()
            .filter(|n| n.ends_with(".tmp"))
            .map(String::as_str)
            .collect()
    }
}
