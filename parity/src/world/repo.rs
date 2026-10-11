use super::World;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Repo {
    pub path: PathBuf,
    pub bare: PathBuf,
    pub url: String,
}

fn git(w: &World, dir: &Path, args: &[&str]) -> String {
    let out = w
        .command("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
    if !out.status.success() {
        panic!(
            "git {args:?} in {} exited {}: {}",
            dir.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

impl Repo {
    pub fn create(w: &World, name: &str) -> Repo {
        let repos = w.root().join("repos");
        let bare = repos.join(format!("{name}.git"));
        let path = repos.join(name);
        let url = format!("https://github.com/acme/{name}");
        fs::create_dir_all(&bare).expect("create bare dir");
        fs::create_dir_all(&path).expect("create work dir");
        let bare_s = bare.to_string_lossy().into_owned();
        git(w, &bare, &["init", "-q", "--bare"]);
        git(w, &path, &["init", "-q", "-b", "main"]);
        fs::write(path.join("README.md"), format!("# {name}\n")).expect("write README");
        git(w, &path, &["add", "README.md"]);
        git(w, &path, &["commit", "-q", "-m", "init"]);
        let key = format!("url.{bare_s}.insteadOf");
        git(w, &path, &["config", "--global", &key, &url]);
        git(w, &path, &["remote", "add", "origin", &url]);
        git(w, &path, &["push", "-q", "origin", "main"]);
        Repo { path, bare, url }
    }

    pub fn head(&self, w: &World) -> String {
        git(w, &self.path, &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }
}
