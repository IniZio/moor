//! Tests for the Box work probe; the script tests run the generated text under `sh` in a scratch repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::{
    boxwork_script, is_full_sha, parse_boxwork, BoxworkCounts, BOXWORK_FILE, BOXWORK_MARKER,
};

const SHA_A: &str = "0123456789abcdef0123456789abcdef01234567";
const SHORT: &str = "0123456789abcdef0123456789abcdef0123456";
const SHA_B: &str = "ABCDEF0123456789abcdef0123456789ABCDEF01";

#[test]
fn full_sha_check() {
    assert!(is_full_sha(SHA_A));
    assert!(is_full_sha(SHA_B));
    assert!(!is_full_sha(""));
    assert!(!is_full_sha(SHORT));
    assert!(!is_full_sha(&format!("{SHA_A}0")));
}

#[test]
fn script_quotes_shas_and_keeps_fragments() {
    let s = boxwork_script(&[SHA_A, SHA_B]).unwrap();
    assert!(s.contains(&format!("'{SHA_A}'")));
    assert!(s.contains(&format!("'{SHA_B}'")));
    assert!(s.contains("--not --remotes $keep"));
    assert!(s.contains("for s in '"));
    assert!(s.contains("--no-renames"));
    assert!(s.contains(BOXWORK_FILE));
    assert!(s.contains(BOXWORK_MARKER));
    assert!(s.starts_with("export GIT_NO_LAZY_FETCH=1\n"));
    assert!(s.contains(&format!("\nrm -f {BOXWORK_FILE}\n")));
}

#[test]
fn script_accepts_empty_saved() {
    let s = boxwork_script(&[]).unwrap();
    assert!(s.contains("for s in; do"));
}

#[test]
fn script_rejects_bad_shas() {
    assert!(boxwork_script(&[SHORT]).is_none());
    assert!(boxwork_script(&[&format!("{SHA_A}0")]).is_none());
    assert!(boxwork_script(&["g123456789abcdef0123456789abcdef01234567"]).is_none());
    assert!(boxwork_script(&["$(touch x)aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]).is_none());
    assert!(boxwork_script(&[&format!("{SHORT}'; rm -rf /; '")]).is_none());
    assert!(boxwork_script(&[SHA_A, "nope"]).is_none());
}

#[test]
fn parse_accepts_file_and_marker_lines() {
    let want = Some(BoxworkCounts { ahead: 2, dirty: 5 });
    assert_eq!(parse_boxwork("ahead=2 dirty=5"), want);
    assert_eq!(parse_boxwork("ahead=2 dirty=5\n"), want);
    assert_eq!(parse_boxwork("  ahead=2 dirty=5  \n\n"), want);
    assert_eq!(parse_boxwork("MOOR_BOXWORK ahead=2 dirty=5"), want);
    assert_eq!(parse_boxwork("out\nMOOR_BOXWORK ahead=2 dirty=5\n"), want);
    assert_eq!(
        parse_boxwork("ahead=0 dirty=0"),
        Some(BoxworkCounts { ahead: 0, dirty: 0 })
    );
}

#[test]
fn parse_rejects_everything_else() {
    for bad in [
        "",
        "\n",
        "garbage",
        "ahead=1",
        "dirty=1",
        "ahead=1 dirty=",
        "ahead= dirty=1",
        "ahead=-1 dirty=0",
        "ahead=+1 dirty=0",
        "ahead=x dirty=0",
        "ahead=1.5 dirty=0",
        "ahead=4294967296 dirty=0",
        "ahead=1 dirty=99999999999",
        "ahead=1 dirty=2 extra",
        "ahead=1 dirty=2 dirty=3",
        "dirty=2 ahead=1",
        "MOOR_BOXWORKahead=1 dirty=2",
        "OTHER ahead=1 dirty=2",
    ] {
        assert_eq!(parse_boxwork(bad), None, "{bad:?}");
    }
}

/// Scratch git repo, removed on drop.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("moor-boxwork-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let repo = Self { root };
        repo.git(&["init", "-q"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Commits `file` with `content` and returns the new HEAD sha.
    fn commit(&self, file: &str, content: &str) -> String {
        self.write(file, content);
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", file]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn probe(&self, saved: &[&str]) -> Output {
        let script = boxwork_script(saved).unwrap();
        Command::new("sh")
            .args(["-c", &script])
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    fn result(&self) -> Option<BoxworkCounts> {
        parse_boxwork(&fs::read_to_string(self.root.join(BOXWORK_FILE)).ok()?)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(Path::new(&self.root));
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn counts_unsaved_commits_and_dirty_files() {
    if !git_available() {
        return;
    }
    let r = Repo::new("counts");
    let base = r.commit("base", "b");
    let head = r.commit("host", "h");
    let pushed = r.commit("pushed", "p");
    r.git(&["update-ref", "refs/remotes/moor/x", &pushed]);
    r.commit("c1", "1");
    r.commit("c2", "2");
    r.write("base", "changed");
    r.write("untracked", "u");
    r.write(".moor/note", "n");

    let out = r.probe(&[&base, &head]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("MOOR_BOXWORK ahead=2 dirty=2"), "{stdout}");
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 2, dirty: 2 }));
}

#[test]
fn saved_sha_history_does_not_count() {
    if !git_available() {
        return;
    }
    let r = Repo::new("saved");
    r.commit("base", "b");
    let head = r.commit("host", "h");
    r.commit("c1", "1");
    assert_eq!(
        parse_boxwork(&String::from_utf8(r.probe(&[&head]).stdout).unwrap()),
        Some(BoxworkCounts { ahead: 1, dirty: 0 })
    );
}

#[test]
fn unknown_saved_sha_is_harmless() {
    if !git_available() {
        return;
    }
    let r = Repo::new("unknown");
    r.commit("base", "b");
    r.commit("c1", "1");
    let out = r.probe(&[SHA_A]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 2, dirty: 0 }));
}

#[test]
fn clean_pushed_repo_is_zero() {
    if !git_available() {
        return;
    }
    let r = Repo::new("clean");
    let tip = r.commit("base", "b");
    r.git(&["update-ref", "refs/remotes/moor/x", &tip]);
    let out = r.probe(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("MOOR_BOXWORK ahead=0 dirty=0"));
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 0, dirty: 0 }));
}

#[test]
fn stale_result_is_removed_and_git_failure_exits_nonzero() {
    if !git_available() {
        return;
    }
    let r = Repo::new("stale");
    r.write(BOXWORK_FILE, "ahead=9 dirty=9\n");
    fs::remove_dir_all(r.root.join(".git")).unwrap();
    // Not a git repo: rev-list fails.
    let out = r.probe(&[]);
    assert!(!out.status.success());
    assert_eq!(r.result(), None);
}

#[test]
fn staged_rename_out_of_moor_is_dirty() {
    if !git_available() {
        return;
    }
    let r = Repo::new("rename");
    r.write(".moor/f", "some content\n");
    r.git(&["add", "-f", ".moor/f"]);
    r.git(&["commit", "-q", "-m", "f"]);
    r.git(&["mv", ".moor/f", "src_f"]);
    let out = r.probe(&[]);
    assert!(out.status.success());
    let counts = r.result().unwrap();
    assert!(counts.dirty >= 1, "{counts:?}");
}

/// Clones a blob-less single-branch copy of a fresh origin, adds one agent commit, then points
/// origin at `url`; returns the clone.
fn partial_clone(name: &str, url: &str) -> Repo {
    let origin = Repo::new(&format!("{name}-origin"));
    origin.git(&["config", "uploadpack.allowFilter", "true"]);
    origin.commit("base", "b");
    let clone = Repo::new(&format!("{name}-clone"));
    fs::remove_dir_all(&clone.root).unwrap();
    let src = format!("file://{}", origin.root.display());
    let out = Command::new("git")
        .args(["clone", "-q", "--filter=blob:none", "--single-branch"])
        .arg(&src)
        .arg(&clone.root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(clone.git(&["config", "remote.origin.promisor"]), "true");
    clone.commit("agent", "a");
    clone.git(&["remote", "set-url", "origin", url]);
    clone
}

fn assert_offline_probe(name: &str, url: &str) {
    let r = partial_clone(name, url);
    let script = boxwork_script(&[SHA_A]).unwrap();
    let mut child = Command::new("sh")
        .args(["-c", &script])
        .current_dir(&r.root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut status = None;
    for _ in 0..200 {
        status = child.try_wait().unwrap();
        if status.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = status.expect("probe did not finish in 10s");
    assert!(status.success());
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 1, dirty: 0 }));
}

#[test]
fn partial_clone_probe_stays_offline_with_silent_origin() {
    if !git_available() {
        return;
    }
    // Accepts connections (kernel backlog) but never answers, like a blackholed address.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x.git", silent.local_addr().unwrap());
    assert_offline_probe("lazy-port", &url);
}

#[test]
fn partial_clone_probe_stays_offline_with_missing_path_origin() {
    if !git_available() {
        return;
    }
    assert_offline_probe("lazy-path", "/nonexistent/moor-boxwork/x.git");
}

#[test]
fn changes_confined_to_cache_dirs_are_not_dirty() {
    if !git_available() {
        return;
    }
    let r = Repo::new("cache");
    for f in [
        ".next/cache/a",
        "apps/web/.next/cache/b",
        "node_modules/.cache/c",
        ".turbo/d",
        "pkg/.pnpm-store/e",
        "src/keep",
    ] {
        r.write(f, "1");
    }
    r.git(&["add", "-f", "."]);
    r.git(&["commit", "-q", "-m", "all"]);
    for f in [
        ".next/cache/a",
        "apps/web/.next/cache/b",
        "node_modules/.cache/c",
        ".turbo/d",
        "pkg/.pnpm-store/e",
    ] {
        fs::remove_file(r.root.join(f)).unwrap();
    }
    r.write("pkg/.pnpm-store/new", "untracked");
    assert!(r.probe(&[]).status.success());
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 1, dirty: 0 }));
}

#[test]
fn source_edits_next_to_cache_dirs_still_count() {
    if !git_available() {
        return;
    }
    let r = Repo::new("cache-source");
    r.write(".next/cache/a", "1");
    r.write("src/keep", "1");
    r.write("src/turbo.rs", "1");
    r.write("deploy/target/x", "1");
    r.git(&["add", "-f", "."]);
    r.git(&["commit", "-q", "-m", "all"]);
    r.write(".next/cache/a", "2");
    r.write("src/keep", "2");
    r.write("src/turbo.rs", "2");
    r.write("deploy/target/x", "2");
    r.write("untracked", "u");
    assert!(r.probe(&[]).status.success());
    assert_eq!(r.result(), Some(BoxworkCounts { ahead: 1, dirty: 4 }));
}
