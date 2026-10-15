//! The Box user's home and the agent's own home.

use moor_core::boxscript::quote;
use moor_core::domain::run::SyncMode;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::LaunchReq;
use moor_core::seam::proc::Deadline;

use super::super::nono_release::NonoRelease;
use super::{fail, install, Failure, INSTALL_DEADLINE};

/// The Box user's home and whether the Box showed its process environment.
pub(super) struct Login {
    /// The Box user's home.
    pub(super) home: BoxPath,
    /// A Linux Box has `/proc`; a Box without it is not one this launch can sandbox unseen.
    pub(super) linux: bool,
}

/// The Box user's home. A Linux Box shows it in the environment of a process; a macOS Box has no
/// such file, so the shell writes it out.
pub(super) fn login(driver: &dyn Driver, req: &LaunchReq) -> Result<Login, Failure> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| fail("home", &e))?;
    let text = driver
        .peek_file(&req.handle, &environ, &req.worktree)
        .map_err(|e| fail("home", &e))?;
    let linux = text.is_some();
    let home = match text {
        Some(text) => text
            .split('\0')
            .find_map(|kv| kv.strip_prefix("HOME="))
            .map(str::to_owned),
        None => Some(from_shell(driver, req)?),
    };
    let home = home.ok_or_else(|| fail("home", &"the Box user has no home directory"))?;
    let home = BoxPath::restore(&home).map_err(|e| fail("home", &e))?;
    Ok(Login { home, linux })
}

/// The home the Box shell reports, through a file beside the workdir that is removed again.
fn from_shell(driver: &dyn Driver, req: &LaunchReq) -> Result<String, Failure> {
    let file = BoxPath::restore(&format!("{}.moor-login-home", req.workdir.as_str()))
        .map_err(|e| fail("home", &e))?;
    let run = |script: String| {
        let spec = ExecSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script],
            env: Vec::new(),
            cwd: None,
            sync: SyncMode::None,
            output: None,
            host_cwd: Some(req.worktree.clone()),
            class: None,
        };
        driver.exec(&req.handle, &spec, Deadline(INSTALL_DEADLINE))
    };
    let at = quote(file.as_str());
    run(format!("printf %s \"$HOME\" > {at}")).map_err(|e| fail("home", &e))?;
    let text = driver.peek_file(&req.handle, &file, &req.worktree);
    let _ = run(format!("rm -f {at}"));
    text.map_err(|e| fail("home", &e))?
        .filter(|h| !h.is_empty())
        .ok_or_else(|| fail("home", &"the Box user has no home directory"))
}

/// The agent's home on `os`: a macOS Box logs in as the owner, whose home holds their own
/// credentials, so the agent gets a directory of its own beside the workdir (outside the git
/// worktree, so it cannot be committed). A Linux Box keeps the login home.
pub(super) fn task(os: &str, workdir: &BoxPath) -> Result<Option<BoxPath>, NameError> {
    if os != "Darwin" {
        return Ok(None);
    }
    task_home(workdir).map(Some)
}

/// The per-Task agent home for `workdir`.
pub(super) fn task_home(workdir: &BoxPath) -> Result<BoxPath, NameError> {
    BoxPath::restore(&format!("{}.moor-home", workdir.as_str()))
}

/// Script lines that create `home` and make it the agent's `HOME`.
pub(super) fn lines(home: &BoxPath) -> [String; 2] {
    let (h, config) = (
        quote(home.as_str()),
        quote(&format!("{}/.config", home.as_str())),
    );
    [
        format!("mkdir -p -m 700 {h} && mkdir -p -m 700 {config} || exit 1"),
        format!("export HOME={h}"),
    ]
}

/// The directories under the agent's home that the profile grants read-write. The home itself is
/// not writable in the sandbox, so each must exist before the agent starts.
pub(super) const GRANTED_DIRS: [&str; 5] = [
    ".claude",
    ".cache",
    ".config/gh",
    ".local/state/claude",
    ".local/share/claude",
];

/// One `mkdir` for `home` and the granted directories. `-m` only applies to the last component of
/// each operand, so every parent is listed ahead of its children to get mode 0700 as well.
fn make_dirs(home: &BoxPath) -> String {
    let mut dirs: Vec<String> = vec![home.as_str().to_owned()];
    for rel in GRANTED_DIRS {
        let mut at = home.as_str().to_owned();
        for part in rel.split('/') {
            at = format!("{at}/{part}");
            if !dirs.contains(&at) {
                dirs.push(at.clone());
            }
        }
    }
    let list: Vec<String> = dirs.iter().map(|d| quote(d)).collect();
    format!("mkdir -p -m 700 {}", list.join(" "))
}

/// Creates the private `home` and the directories the sandbox can write, before the profile's
/// files are written into it.
pub(super) fn create(driver: &dyn Driver, req: &LaunchReq, home: &BoxPath) -> Result<(), Failure> {
    let make = make_dirs(home);
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), make],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(req.worktree.clone()),
        class: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(INSTALL_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(_) => Err(fail("home", &"could not create the agent's home")),
        Err(e) => Err(fail("home", &e)),
    }
}

/// The Box OS and the agent's own home. A launch with a Proxy checks the sandbox tool, which tells
/// the OS, and on macOS creates the agent's home before the profile writes into it. A launch
/// without one is refused unless the Box is Linux, since the agent would run in the owner's home.
pub(super) fn agent(
    driver: &dyn Driver,
    req: &LaunchReq,
    login: &Login,
    cred: &BoxPath,
    release: &NonoRelease,
) -> Result<(Option<String>, Option<BoxPath>), Failure> {
    if req.proxy.is_none() {
        let why = "the Box is not Linux, and only a sandboxed agent may run there";
        return if login.linux {
            Ok((None, None))
        } else {
            Err(fail("sandbox", &why))
        };
    }
    let os = install::ensure(driver, req, (&login.home, cred), release)?;
    let home = task(&os, &req.workdir).map_err(|e| fail("sandbox", &e))?;
    if let Some(h) = &home {
        create(driver, req, h)?;
    }
    Ok((Some(os), home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::box_agent::sandbox::{profile, SandboxInput};

    fn path(p: &str) -> BoxPath {
        BoxPath::restore(p).unwrap()
    }

    /// Every directory the profile lets the agent write under its home exists, private, before
    /// the sandbox starts: the home itself is not writable there.
    #[test]
    fn every_granted_directory_is_made_private_ahead_of_the_sandbox() {
        let home = path("/work/t.moor-home");
        let input = SandboxInput {
            os: "Darwin",
            workdir: &path("/work/t"),
            home: &home,
            login_home: &path("/Users/o"),
            cred: &path("/cred"),
            tmp: &path("/work/t.tmp"),
            env_names: Vec::new(),
            set_vars: Vec::new(),
        };
        let doc: serde_json::Value = serde_json::from_str(&profile(&input)).unwrap();
        let allow = doc
            .pointer("/filesystem/allow")
            .unwrap()
            .as_array()
            .unwrap();
        let prefix = format!("{}/", home.as_str());
        let granted: Vec<&str> = (allow.iter().filter_map(|v| v.as_str()))
            .filter(|a| a.starts_with(&prefix))
            .collect();
        assert_eq!(granted.len(), GRANTED_DIRS.len(), "{granted:?}");
        let make = make_dirs(&home);
        assert!(make.starts_with("mkdir -p -m 700 "), "{make}");
        let words: Vec<&str> = make.split(' ').skip(4).collect();
        for dir in granted {
            let mut at = dir;
            while at != home.as_str() {
                assert!(words.contains(&format!("'{at}'").as_str()), "{at}: {make}");
                at = at.rsplit_once('/').unwrap().0;
            }
        }
    }
}
