//! `moor controller serve` under socket activation: a holder binds the sockets and starts the
//! controller with `LISTEN_FDS`, `LISTEN_PID` and `LISTEN_FDNAMES`, as systemd does.

use std::fs::{self, File};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use command_fds::{CommandFdExt as _, FdMapping};
use moor_core::controller::PROTOCOL;
use serde_json::Value;

use super::fresh_dir;

/// Holds the controller socket and the `web` front's socket across controller runs.
struct Holder {
    root: super::Dir,
    config: PathBuf,
    socket: PathBuf,
    front: PathBuf,
    listeners: Vec<OwnedFd>,
}

impl Holder {
    fn new() -> Self {
        let root = fresh_dir("held");
        let socket = root.join("c.sock");
        let state = root.join("state");
        let front = state.join("fronts/web/front");
        let config = root.join("c.yaml");
        let yaml = format!(
            "socket: {}\nstate_dir: {}\nfronts:\n  web: {{}}\n",
            socket.display(),
            state.display()
        );
        fs::write(&config, yaml).unwrap();
        fs::create_dir_all(front.parent().unwrap()).unwrap();
        let listeners = vec![listen(&socket), listen(&front)];
        Self {
            root,
            config,
            socket,
            front,
            listeners,
        }
    }

    /// Binds one more socket in the root, for a holder that passes more than the config uses.
    fn add(&mut self, name: &str) {
        self.listeners.push(listen(&self.root.join(name)));
    }

    /// Starts the controller with every held socket passed in under `names`, the way systemd
    /// does: `LISTEN_PID` is the controller's own pid.
    fn serve(&self, names: &str) -> Child {
        self.serve_as(None, &self.listeners.len().to_string(), names)
    }

    /// Starts the controller with the sockets passed in, but `LISTEN_PID` naming another process.
    fn serve_for_another_pid(&self, names: &str) -> Child {
        let count = self.listeners.len().to_string();
        self.serve_as(Some(&std::process::id().to_string()), &count, names)
    }

    /// Starts the controller with every held socket passed in, `LISTEN_PID` set to `pid` (the
    /// controller's own when `None`), `LISTEN_FDS` to `count` and `LISTEN_FDNAMES` to `names`.
    fn serve_as(&self, pid: Option<&str>, count: &str, names: &str) -> Child {
        let mut cmd = pid.map_or_else(
            || {
                let mut cmd = self.command("/bin/sh");
                cmd.args(["-c", r#"export LISTEN_PID=$$; exec "$0" "$@""#])
                    .arg(env!("CARGO_BIN_EXE_moor"));
                cmd
            },
            |pid| {
                let mut cmd = self.command(env!("CARGO_BIN_EXE_moor"));
                cmd.env("LISTEN_PID", pid);
                cmd
            },
        );
        let mappings = (3..)
            .zip(&self.listeners)
            .map(|(child_fd, l)| FdMapping {
                parent_fd: l.try_clone().unwrap(),
                child_fd,
            })
            .collect();
        cmd.fd_mappings(mappings).unwrap();
        cmd.args(["controller", "serve", "--config"])
            .arg(&self.config)
            .env("LISTEN_FDS", count)
            .env("LISTEN_FDNAMES", names)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(self.root.join("stderr")).unwrap())
            .spawn()
            .unwrap()
    }

    /// `program` in the root with a clean environment.
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"));
        cmd
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.root.join("stderr")).unwrap()
    }
}

const NAMES: &str = "controller:front-web";

/// An owner-only listener at `path`, as `SocketMode=0600` makes it.
fn listen(path: &Path) -> OwnedFd {
    let listener = UnixListener::bind(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    listener.into()
}

fn inode(path: &Path) -> u64 {
    fs::metadata(path).unwrap().ino()
}

/// Sends hello and a `version` request on `socket` without reading, so it works before any
/// controller runs.
fn ask_version(socket: &Path) -> BufReader<UnixStream> {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#).unwrap();
    writeln!(
        stream,
        r#"{{"frame":"request","id":"1","request":{{"cmd":"version"}}}}"#
    )
    .unwrap();
    BufReader::new(stream)
}

/// Reads the hello reply and the version response, returning the response's class.
fn answer(reader: &mut BufReader<UnixStream>) -> String {
    let mut next = || {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let hello = next();
    assert_eq!(
        hello.pointer("/frame"),
        Some(&Value::from("hello")),
        "{hello}"
    );
    let response = next();
    assert_eq!(
        response.pointer("/frame"),
        Some(&Value::from("response")),
        "{response}"
    );
    let class = response.pointer("/envelope/class").and_then(Value::as_str);
    class.unwrap().to_owned()
}

/// SIGTERMs `child`, which drains, and returns its exit code.
fn stop(child: &mut Child) -> Option<i32> {
    let pid = rustix::process::Pid::from_child(child);
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    exit_code(child)
}

/// Exit code of `child`, or `None` after killing it when it runs past five seconds.
fn exit_code(child: &mut Child) -> Option<i32> {
    for _ in 0..500 {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn a_connect_made_before_the_controller_starts_is_answered() {
    let holder = Holder::new();
    let mut early = ask_version(&holder.socket);
    let mut early_front = ask_version(&holder.front);
    let mut controller = holder.serve(NAMES);
    assert_eq!(answer(&mut early), "ok", "{}", holder.stderr());
    assert_eq!(answer(&mut early_front), "ok", "{}", holder.stderr());
    assert_eq!(stop(&mut controller), Some(0), "{}", holder.stderr());
}

#[test]
fn the_held_sockets_keep_their_inodes_across_a_controller_restart() {
    let holder = Holder::new();
    let before = (inode(&holder.socket), inode(&holder.front));
    for _ in 0..2 {
        let mut controller = holder.serve(NAMES);
        assert_eq!(
            answer(&mut ask_version(&holder.socket)),
            "ok",
            "{}",
            holder.stderr()
        );
        assert_eq!(
            answer(&mut ask_version(&holder.front)),
            "ok",
            "{}",
            holder.stderr()
        );
        assert_eq!(stop(&mut controller), Some(0), "{}", holder.stderr());
        assert_eq!((inode(&holder.socket), inode(&holder.front)), before);
    }
}

#[test]
fn passed_sockets_are_close_on_exec_in_the_controller() {
    let holder = Holder::new();
    let mut controller = holder.serve(NAMES);
    assert_eq!(
        answer(&mut ask_version(&holder.socket)),
        "ok",
        "{}",
        holder.stderr()
    );
    let proc = PathBuf::from(format!("/proc/{}", controller.id()));
    let inherited: Vec<String> = fs::read_dir(proc.join("fd"))
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            let target = fs::read_link(e.path()).ok()?;
            target.to_str()?.starts_with("socket:").then_some(())?;
            let fd = e.file_name().into_string().unwrap();
            let info = fs::read_to_string(proc.join("fdinfo").join(&fd)).ok()?;
            let flags = info.lines().find_map(|l| l.strip_prefix("flags:"))?.trim();
            let cloexec = u32::from_str_radix(flags, 8).unwrap() & 0o2_000_000 != 0;
            (!cloexec).then_some(fd)
        })
        .collect();
    assert_eq!(
        inherited,
        Vec::<String>::new(),
        "sockets a child would inherit"
    );
    assert_eq!(stop(&mut controller), Some(0));
}

#[test]
fn a_passed_socket_the_config_does_not_use_stops_the_start() {
    let mut holder = Holder::new();
    holder.add("x.sock");
    let mut controller = holder.serve("controller:front-web:stray");
    assert_eq!(exit_code(&mut controller), Some(1));
    let stderr = holder.stderr();
    assert!(stderr.contains("stray"), "{stderr}");
}

#[test]
fn sockets_passed_under_swapped_names_stop_the_start() {
    let holder = Holder::new();
    let mut controller = holder.serve("front-web:controller");
    assert_eq!(exit_code(&mut controller), Some(1));
    let stderr = holder.stderr();
    assert!(stderr.contains("web/front"), "{stderr}");
}

#[test]
fn a_passed_socket_open_to_its_group_or_to_everyone_stops_the_start() {
    for mode in [0o660, 0o606] {
        let holder = Holder::new();
        fs::set_permissions(&holder.front, fs::Permissions::from_mode(mode)).unwrap();
        let mut controller = holder.serve(NAMES);
        assert_eq!(exit_code(&mut controller), Some(1), "mode {mode:o}");
        let stderr = holder.stderr();
        assert!(
            stderr.contains("SocketMode=0600"),
            "mode {mode:o}: {stderr}"
        );
    }
}

/// Starts the controller from `holder` with `LISTEN_PID`, `LISTEN_FDS` and `LISTEN_FDNAMES`
/// as given, and returns its stderr once it has stopped with exit 1.
fn refused(holder: &Holder, pid: Option<&str>, count: &str, names: &str) -> String {
    let mut controller = holder.serve_as(pid, count, names);
    let code = exit_code(&mut controller);
    let stderr = holder.stderr();
    assert_eq!(code, Some(1), "{stderr}");
    stderr
}

#[test]
fn a_needed_socket_not_passed_stops_the_start_without_binding_it() {
    let mut holder = Holder::new();
    let held = inode(&holder.front);
    holder.listeners.truncate(1);
    let stderr = refused(&holder, None, "1", "controller");
    assert!(stderr.contains("no socket named front-web"), "{stderr}");
    assert_eq!(
        inode(&holder.front),
        held,
        "the front's path was bound again"
    );
}

#[test]
fn a_malformed_listen_pid_or_listen_fds_stops_the_start() {
    let holder = Holder::new();
    let stderr = refused(&holder, Some("me"), "2", NAMES);
    assert!(stderr.contains("LISTEN_PID is not a pid"), "{stderr}");
    let stderr = refused(&holder, None, "two", NAMES);
    assert!(stderr.contains("LISTEN_FDS is not a count"), "{stderr}");
}

#[test]
fn listen_fdnames_naming_a_different_count_stops_the_start() {
    let holder = Holder::new();
    let stderr = refused(&holder, None, "2", "controller");
    assert!(
        stderr.contains("LISTEN_FDNAMES names 1 sockets"),
        "{stderr}"
    );
}

#[test]
fn a_name_passed_twice_stops_the_start() {
    let holder = Holder::new();
    let stderr = refused(&holder, None, "2", "controller:controller");
    assert!(
        stderr.contains("socket controller is passed twice"),
        "{stderr}"
    );
}

#[test]
fn a_passed_socket_that_is_not_a_listening_stream_stops_the_start() {
    let mut holder = Holder::new();
    fs::remove_file(&holder.socket).unwrap();
    let datagram = UnixDatagram::bind(&holder.socket).unwrap();
    fs::set_permissions(&holder.socket, fs::Permissions::from_mode(0o600)).unwrap();
    *holder.listeners.first_mut().unwrap() = datagram.into();
    let stderr = refused(&holder, None, "2", NAMES);
    assert!(stderr.contains("not a listening stream socket"), "{stderr}");
}

/// An owner-only socket of `kind` bound at `path`, listening when `listening` is set.
fn bound(path: &Path, kind: rustix::net::SocketType, listening: bool) -> OwnedFd {
    use rustix::net::{bind, listen, socket_with, AddressFamily, SocketAddrUnix, SocketFlags};
    // Close-on-exec, as std makes its sockets, so a controller another test spawns meanwhile
    // does not inherit it.
    let fd = socket_with(AddressFamily::UNIX, kind, SocketFlags::CLOEXEC, None).unwrap();
    bind(&fd, &SocketAddrUnix::new(path).unwrap()).unwrap();
    if listening {
        listen(&fd, 8).unwrap();
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    fd
}

#[test]
fn a_passed_listening_seqpacket_socket_or_a_stream_that_does_not_listen_stops_the_start() {
    use rustix::net::SocketType;
    for (kind, listening) in [(SocketType::SEQPACKET, true), (SocketType::STREAM, false)] {
        let mut holder = Holder::new();
        fs::remove_file(&holder.socket).unwrap();
        *holder.listeners.first_mut().unwrap() = bound(&holder.socket, kind, listening);
        let stderr = refused(&holder, None, "2", NAMES);
        assert!(
            stderr.contains("not a listening stream socket"),
            "{kind:?} listening={listening}: {stderr}"
        );
    }
}

#[test]
fn listen_fdnames_naming_more_sockets_than_listen_fds_stops_the_start() {
    let holder = Holder::new();
    let stderr = refused(&holder, None, "1", NAMES);
    assert!(
        stderr.contains("LISTEN_FDNAMES names 2 sockets but LISTEN_FDS passes 1"),
        "{stderr}"
    );
}

#[test]
fn sockets_passed_to_another_pid_are_ignored_and_the_controller_binds_its_own() {
    let holder = Holder::new();
    let held = inode(&holder.socket);
    let mut controller = holder.serve_for_another_pid(NAMES);
    // The controller's own bind replaces the held path; poll until it has.
    for _ in 0..500 {
        if fs::metadata(&holder.socket).is_ok_and(|m| m.ino() != held) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_ne!(inode(&holder.socket), held, "{}", holder.stderr());
    assert_eq!(
        answer(&mut ask_version(&holder.socket)),
        "ok",
        "{}",
        holder.stderr()
    );
    assert_eq!(stop(&mut controller), Some(0));
}
