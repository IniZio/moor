//! Sets up and starts the person's rootless daemon with Docker's own tools.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use super::probe::answers;
use super::{Daemon, Facts, Shell, AGAIN};

/// Docker's installer of rootless Docker under ~/bin, for a host without Docker's package.
const GET_ROOTLESS: &str = "https://get.docker.com/rootless";
/// The daemon's `PATH` after Docker's own directory, so an older rootlesskit elsewhere loses.
const SYSTEM_PATH: &str = "/usr/bin:/usr/sbin:/bin:/sbin";
/// How long a daemon that was just started gets to answer.
const WAIT_SECS: u32 = 30;

/// Sets up and starts the person's rootless daemon where it is not running.
pub(super) fn install(
    f: &Facts,
    shell: &mut dyn Shell,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    match f.daemon {
        Daemon::Answers => say("rootless docker: ok"),
        Daemon::Installed { active: true } => say(&format!(
            "rootless docker: docker.service runs, but nothing answers at {} yet",
            f.socket.display()
        )),
        Daemon::Installed { active: false } => {
            start(f, shell)?;
            say("rootless docker: started");
        }
        Daemon::Absent => fresh(f, shell, say)?,
    }
    Ok(())
}

/// Installs rootless Docker with Docker's setup tool, or Docker's rootless installer where the
/// tool is missing, after writing the daemon's settings so its first start has them, and puts
/// the CLI's context back, since the tool switches it.
fn fresh(f: &Facts, shell: &mut dyn Shell, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let (bin, tool) = f.setuptool.as_ref().map_or_else(
        || (f.home.join("bin"), GET_ROOTLESS.to_owned()),
        |p| {
            let dir = p.parent().map_or_else(PathBuf::new, Path::to_path_buf);
            (dir, p.display().to_string())
        },
    );
    let path = if bin == Path::new("/usr/bin") {
        SYSTEM_PATH.to_owned()
    } else {
        format!("{}:{SYSTEM_PATH}", bin.display())
    };
    settings(f, &path)?;
    let before = context(&f.docker_config);
    let stock = [
        ("PATH", Some(path.as_str())),
        ("DOCKER_HOST", None),
        ("DOCKER_CONTEXT", None),
    ];
    let ran = if f.setuptool.is_some() {
        shell.run(&tool, &["install", "--force"], &stock)
    } else {
        let script = format!("curl -fsSL {GET_ROOTLESS} | sh");
        let env = [stock.as_slice(), &[("FORCE_ROOTLESS_INSTALL", Some("1"))]].concat();
        shell.run("sh", &["-c", &script], &env)
    };
    restore(f, before.as_deref(), shell, say);
    if !ran.ok {
        return Err(format!(
            "{tool} failed:\n{}\nfix the error above, {AGAIN}",
            ran.out.trim()
        ));
    }
    start(f, shell)?;
    say(&format!("rootless docker: installed with {tool}"));
    Ok(())
}

/// Writes slirp4netns and `path` into a drop-in of the daemon's unit, and live-restore off into
/// the person's daemon.json, keeping what else it says.
fn settings(f: &Facts, path: &str) -> Result<(), String> {
    let json = f.config.join("docker/daemon.json");
    let mut doc = match fs::read_to_string(&json) {
        Ok(text) => serde_json::from_str::<Map<String, Value>>(&text).map_err(|e| {
            format!(
                "{} is not a JSON object: {e}; fix it, {AGAIN}",
                json.display()
            )
        })?,
        Err(e) if e.kind() == ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", json.display())),
    };
    if doc.get("live-restore") != Some(&Value::Bool(false)) {
        doc.insert("live-restore".to_owned(), Value::Bool(false));
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        keep(&json, &format!("{text}\n"))?;
    }
    keep(
        &f.config.join("systemd/user/docker.service.d/moor.conf"),
        &format!(
            "[Service]\nEnvironment=DOCKERD_ROOTLESS_ROOTLESSKIT_NET=slirp4netns\nEnvironment=PATH={path}\n"
        ),
    )
}

/// Writes `text` to `file`, making its directory.
fn keep(file: &Path, text: &str) -> Result<(), String> {
    file.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(file, text))
        .map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// The docker CLI's current context, from its config in `dir`; none means `default`.
fn context(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("config.json")).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    doc.get("currentContext")?.as_str().map(str::to_owned)
}

/// Switches the docker CLI back to `before` where it moved.
fn restore(f: &Facts, before: Option<&str>, shell: &mut dyn Shell, say: &mut dyn FnMut(&str)) {
    if context(&f.docker_config).as_deref() == before {
        return;
    }
    let name = before.unwrap_or("default");
    let ran = shell.run(
        "docker",
        &["context", "use", name],
        &[("DOCKER_HOST", None), ("DOCKER_CONTEXT", None)],
    );
    if !ran.ok {
        say(&format!(
            "warning: the docker CLI moved to another context and could not move back: {}; fix: run `docker context use {name}`",
            ran.out.trim()
        ));
    }
}

/// Enables and starts the daemon's unit, then waits for the daemon to answer.
fn start(f: &Facts, shell: &mut dyn Shell) -> Result<(), String> {
    let ran = shell.run("systemctl", &["--user", "enable", "--now", "docker"], &[]);
    if !ran.ok {
        return Err(format!(
            "systemctl --user enable --now docker failed:\n{}\nsee journalctl --user -u docker, fix it, {AGAIN}",
            ran.out.trim()
        ));
    }
    for _ in 0..WAIT_SECS {
        if answers(shell, &f.socket) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!(
        "rootless Docker did not answer at {} within {WAIT_SECS}s; see journalctl --user -u docker, fix it, {AGAIN}",
        f.socket.display()
    ))
}
