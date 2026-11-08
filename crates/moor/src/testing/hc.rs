//! A herdr-core core of its own, standing in for the one on a Box: its `bin` directory holds a
//! `herdr-core` that speaks only to this core, so a Box command finds it on `PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serializes the cores, which share the process's pty budget.
static SERIAL: Mutex<()> = Mutex::new(());

/// A private core, stopped on drop.
pub(crate) struct Core {
    /// The core's home: its state and the wrapper.
    dir: tempfile::TempDir,
    /// Held while the core lives.
    _serial: MutexGuard<'static, ()>,
}

impl Core {
    /// A core of its own; `None` when the host has no herdr-core.
    pub(crate) fn start() -> Option<Self> {
        let home = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/bin"));
        let real = std::env::split_paths(&std::env::var_os("PATH")?)
            .chain(home)
            .map(|d| d.join("herdr-core"))
            .find(|p| p.is_file())?;
        let dir = tempfile::Builder::new()
            .prefix("hc")
            .tempdir_in("/tmp")
            .ok()?;
        std::fs::create_dir_all(dir.path().join("bin")).ok()?;
        std::fs::create_dir(dir.path().join("run")).ok()?;
        let d = dir.path().display();
        let script = format!(
            "#!/bin/sh\nunset HC_SOCKET HC_SESSION HC_TERMINAL_ID HC_TERMINAL_SOCKET HC_SCOPE\n\
             export HOME={d} XDG_RUNTIME_DIR={d}/run XDG_STATE_HOME={d}/state \
             XDG_CONFIG_HOME={d}/config XDG_DATA_HOME={d}/data XDG_CACHE_HOME={d}/cache\n\
             exec {} \"$@\"\n",
            real.display()
        );
        // Written by a child: a writable fd in this process leaks into other tests' forks and
        // makes exec of the wrapper fail with ETXTBSY.
        let draft = dir.path().join("herdr-core.sh");
        std::fs::write(&draft, script).ok()?;
        let installed = Command::new("install")
            .args(["-m", "755"])
            .arg(&draft)
            .arg(dir.path().join("bin/herdr-core"))
            .status()
            .ok()?;
        assert!(installed.success(), "install the wrapper");
        Some(Self {
            dir,
            _serial: SERIAL.lock().unwrap_or_else(PoisonError::into_inner),
        })
    }

    /// The directory holding this core's `herdr-core`.
    pub(crate) fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    /// `PATH` with this core's `herdr-core` first.
    pub(crate) fn path(&self) -> String {
        format!(
            "{}:{}",
            self.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// Runs herdr-core on this core and returns its trimmed output.
    pub(crate) fn hc(&self, args: &[&str]) -> String {
        let out = Command::new(self.bin().join("herdr-core"))
            .args(args)
            .output()
            .expect("herdr-core");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// The value of doc `name`, `null` when it was never written.
    pub(crate) fn doc(&self, name: &str) -> serde_json::Value {
        let call = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "doc.get", "params": { "name": name }
        });
        let out = Command::new("sh")
            .args([
                "-c",
                "printf '%s\\n' \"$1\" | \"$2\" connect",
                "doc",
                &call.to_string(),
            ])
            .arg(self.bin().join("herdr-core"))
            .output()
            .expect("herdr-core connect");
        let reply: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("a doc.get reply");
        reply
            .pointer("/result/value")
            .cloned()
            .expect("a doc value")
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.hc(&["server", "stop"]);
    }
}

/// The core for a test, or a skip note when the host has no herdr-core.
pub(crate) fn core() -> Option<Core> {
    let core = Core::start();
    if core.is_none() {
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            b"skipped: herdr-core is not installed\n",
        );
    }
    core
}
