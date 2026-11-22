//! A stand-in for a rootless Docker daemon whose every preflight check passes, for the tests of
//! `moor up` and `moor-machine-agent up`.

use std::io::{self, Read as _, Write as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{write_executable, ChildGuard};

/// The address the stand-in's rootlesskit reports for its namespace.
pub const CHILD_IP: &str = "10.0.2.100";

/// What `docker`, `rootlesskit`'s state dir and the host files say on a rootless host that is
/// ready for Boxes. Its rootlesskit child lives as long as this does.
#[derive(Debug)]
pub struct Rootless {
    /// Holds the daemon's socket path, rootlesskit's state dir and the host files.
    dir: tempfile::TempDir,
    /// The process `child_pid` names, whose command line carries `--disable-host-loopback`.
    _child: ChildGuard,
}

impl Rootless {
    /// Makes the stand-in, with the programs rootless Docker and the relay need in `bin`.
    pub fn start(bin: &Path) -> io::Result<Self> {
        let dir = crate::short_tempdir("rl")?;
        let state = dir.path().join("dockerd-rootless");
        std::fs::create_dir_all(&state)?;
        let child = ChildGuard::new(
            Command::new("sh")
                .args(["-c", "read x", "rootlesskit", "--disable-host-loopback"])
                .stdin(Stdio::piped())
                .spawn()?,
        );
        std::fs::write(state.join("child_pid"), format!("{}\n", child.id()))?;
        serve_info(&state.join("api.sock"))?;
        let uid = rustix::process::getuid().as_raw();
        let etc = dir.path().join("root/etc");
        std::fs::create_dir_all(&etc)?;
        for file in ["subuid", "subgid"] {
            std::fs::write(etc.join(file), format!("{uid}:100000:65536\n"))?;
        }
        let sysctl = dir.path().join("root/proc/sys/kernel");
        std::fs::create_dir_all(&sysctl)?;
        std::fs::write(sysctl.join("apparmor_restrict_unprivileged_userns"), "0\n")?;
        for tool in ["newuidmap", "newgidmap", "slirp4netns", "nsenter"] {
            write_executable(&bin.join(tool), "#!/bin/sh\nexit 0\n")?;
        }
        write_executable(&bin.join("loginctl"), "#!/bin/sh\necho Linger=yes\n")?;
        Ok(Self { dir, _child: child })
    }

    /// The root to give `MOOR_HOST_ROOT`, which holds the id files and the `AppArmor` sysctl.
    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    /// rootlesskit's state dir.
    #[must_use]
    pub fn state(&self) -> PathBuf {
        self.dir.path().join("dockerd-rootless")
    }

    /// The `case "$*" in` arms of a `docker` stand-in that answers as this rootless daemon.
    #[must_use]
    pub fn docker_arms(&self) -> String {
        format!(
            "  \"info --format {{{{json .SecurityOptions}}}}\") echo '[\"name=seccomp,profile=builtin\",\"name=rootless\"]' ;;\n  \
             \"info --format {{{{.LiveRestoreEnabled}}}}\") echo false ;;\n  \
             \"context inspect --format {{{{.Endpoints.docker.Host}}}}\") echo unix://{} ;;\n",
            self.dir.path().join("docker.sock").display()
        )
    }
}

/// Answers rootlesskit's `/v1/info` on `socket` for as long as the test runs.
fn serve_info(socket: &Path) -> io::Result<()> {
    let listener = UnixListener::bind(socket)?;
    let body =
        format!("{{\"networkDriver\":{{\"driver\":\"slirp4netns\",\"childIP\":\"{CHILD_IP}\"}}}}");
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut head = [0_u8; 1024];
            let _ = conn.read(&mut head);
            let _ = write!(conn, "HTTP/1.0 200 OK\r\n\r\n{body}");
        }
    });
    Ok(())
}
