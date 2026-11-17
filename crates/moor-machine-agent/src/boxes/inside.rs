//! Commands and port forwards in a Box, over the provider's carrier: a command that runs one
//! shell script in a place. Nothing here knows how the provider reaches the place.
//!
//! An `exec` stream carries [`chan`] frames. The controller ends stdin with an `in-end` frame,
//! never by closing its half of the stream: a stream that ends before the command exits means
//! the controller is gone, so the command's process group is ended (TERM, then KILL after 5 s).

use std::io;
use std::os::fd::OwnedFd;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use moor_client::link::boxes::Size;
use moor_client::link::chan::{self, Exit, Frame};
use moor_client::link::ops;
use moor_core::seam::driver::Place;
use rustix::process::{kill_process_group, Pid, Signal};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

mod pty;

/// The exit code a place's carrier uses to say the place is down, which reaches the controller
/// as [`Exit::Down`] for the work place.
const DOWN_CODE: i32 = 75;
/// How long output may keep arriving after the command exits, from children it left.
const DRAIN: Duration = Duration::from_secs(2);
/// How long a command has to end after TERM before KILL.
const GRACE: Duration = Duration::from_secs(5);
/// Bytes read from the command at once.
const CHUNK: usize = 16 * 1024;

/// A stream's source of the command's bytes.
type Source = Box<dyn AsyncRead + Unpin + Send>;
/// A sink for the command's stdin.
type Sink = Box<dyn AsyncWrite + Unpin + Send>;

/// The shell script that runs `argv` in `cwd` with `env` exported.
pub(super) fn script(env: &[(String, String)], cwd: Option<&str>, argv: &[String]) -> String {
    let go = cwd.map_or_else(String::new, |dir| {
        format!("cd {} || exit 1\n", ops::quote(dir))
    });
    let words: Vec<String> = argv.iter().map(|a| ops::quote(a)).collect();
    ops::script(env, &format!("{go}exec {}", words.join(" ")))
}

/// A command started in a place, before its stream carries anything.
pub(super) struct Started {
    /// The carrier.
    child: Child,
    /// Its stdout; the terminal when it has one.
    out: Source,
    /// Its stderr; none on a terminal.
    err: Option<Source>,
    /// Its stdin; none when the request sent none.
    stdin: Option<Sink>,
    /// The terminal's master, to resize.
    master: Option<OwnedFd>,
}

/// Starts `carrier` on `script`, on a terminal of size `tty` when given.
pub(super) fn start(
    carrier: Command,
    script: &str,
    stdin: bool,
    tty: Option<Size>,
) -> io::Result<Started> {
    match tty {
        Some(size) => on_terminal(&carrier, script, size),
        None => piped(carrier, script, stdin),
    }
}

/// `carrier` with pipes for stdio, as the leader of a process group.
fn piped(mut carrier: Command, script: &str, stdin: bool) -> io::Result<Started> {
    carrier
        .arg(script)
        .process_group(0)
        .kill_on_drop(true)
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = carrier.spawn()?;
    let lost = || io::Error::other("the command has no pipe");
    Ok(Started {
        out: Box::new(child.stdout.take().ok_or_else(lost)?),
        err: Some(Box::new(child.stderr.take().ok_or_else(lost)?)),
        stdin: child.stdin.take().map(|s| Box::new(s) as Sink),
        master: None,
        child,
    })
}

/// `carrier` on a new pty of `size`, as the leader of a session that owns it.
fn on_terminal(carrier: &Command, script: &str, size: Size) -> io::Result<Started> {
    let (master, slave) = pty::open(size)?;
    let [stdin, stdout, stderr] = pty::stdio(slave)?;
    let mut cmd = pty::controlled(carrier);
    cmd.arg(script)
        .kill_on_drop(true)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    let child = cmd.spawn()?;
    // Our copies of the slave close here; otherwise the master never sees the command end.
    drop(cmd);
    Ok(Started {
        out: Box::new(pty::file(&master)?),
        err: None,
        stdin: Some(Box::new(pty::file(&master)?)),
        master: Some(master),
        child,
    })
}

/// Carries the started command's stdio over `stream` as frames until it exits or the controller
/// is gone, and ends with its `exit` frame. `place` says whether code 75 means down.
pub(super) async fn pump<S>(started: Started, place: Place, stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Started {
        mut child,
        mut out,
        mut err,
        stdin,
        master,
    } = started;
    let (rd, mut wr) = tokio::io::split(stream);
    let eot = master.is_some();
    let (feed_tx, feed_rx) = mpsc::channel(8);
    if let Some(sink) = stdin {
        tokio::spawn(feed(sink, feed_rx, eot));
    }
    let mut client = tokio::spawn(read_client(rd, feed_tx, master));
    let ended = forward(&mut child, (&mut out, err.as_mut()), &mut wr, &mut client).await;
    client.abort();
    match ended {
        Ended::Exited(status) => {
            drain(&mut out, err.as_mut(), &mut wr).await;
            let exit = exit_of(status, place);
            let _ = chan::write_frame(&mut wr, &Frame::Exit(exit)).await;
            let _ = wr.shutdown().await;
        }
        Ended::Gone => end_group(&mut child).await,
    }
}

/// How the command's carrier stopped, or that the controller did first.
enum Ended {
    /// The carrier exited.
    Exited(ExitStatus),
    /// The controller's stream ended, or could not be written.
    Gone,
}

/// Copies the command's output to `wr` as frames until the carrier exits or `client` ends.
async fn forward<W>(
    child: &mut Child,
    (out, err): (&mut Source, Option<&mut Source>),
    wr: &mut W,
    client: &mut tokio::task::JoinHandle<()>,
) -> Ended
where
    W: AsyncWrite + Unpin,
{
    let mut err = err;
    let (mut out_open, mut err_open) = (true, err.is_some());
    let (mut ob, mut eb) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    loop {
        let sent = tokio::select! {
            n = read_some(out, &mut ob), if out_open => {
                out_open = n > 0;
                send(wr, Frame::Out, ob.get(..n)).await
            }
            n = read_err(err.as_deref_mut(), &mut eb), if err_open => {
                err_open = n > 0;
                send(wr, Frame::Err, eb.get(..n)).await
            }
            status = child.wait() => match status {
                Ok(status) => return Ended::Exited(status),
                Err(_) => return Ended::Gone,
            },
            _ = &mut *client => false,
        };
        if !sent {
            return Ended::Gone;
        }
    }
}

/// Reads what `src` has, or 0 at its end or failure; a terminal's master fails with `EIO` once
/// the command's side closes.
async fn read_some<R: AsyncRead + Unpin + ?Sized>(src: &mut R, buf: &mut [u8]) -> usize {
    src.read(buf).await.unwrap_or(0)
}

/// [`read_some`] of the command's stderr, when it has one.
async fn read_err(src: Option<&mut Source>, buf: &mut [u8]) -> usize {
    match src {
        Some(src) => read_some(src, buf).await,
        None => 0,
    }
}

/// Writes `bytes` as a frame made by `make`; false when the stream is closed. Nothing to write
/// is a success.
async fn send<'a, W: AsyncWrite + Unpin>(
    wr: &mut W,
    make: fn(&'a [u8]) -> Frame<'a>,
    bytes: Option<&'a [u8]>,
) -> bool {
    match bytes {
        Some(b) if !b.is_empty() => chan::write_frame(wr, &make(b)).await.is_ok(),
        _ => true,
    }
}

/// Copies what the exited command's children still write, for a moment.
async fn drain<W: AsyncWrite + Unpin>(out: &mut Source, err: Option<&mut Source>, wr: &mut W) {
    let rest = async {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = read_some(out, &mut buf).await;
            if n == 0 || !send(wr, Frame::Out, buf.get(..n)).await {
                break;
            }
        }
        let Some(err) = err else { return };
        loop {
            let n = read_some(err, &mut buf).await;
            if n == 0 || !send(wr, Frame::Err, buf.get(..n)).await {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(DRAIN, rest).await;
}

/// How `status` reads to the controller.
fn exit_of(status: ExitStatus, place: Place) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(DOWN_CODE), _) if place == Place::Work => Exit::Down,
        (Some(code), _) => Exit::Code(code),
        (None, Some(signal)) => Exit::Signal(signal),
        (None, None) => Exit::Code(-1),
    }
}

/// Reads the controller's frames: stdin to `feed`, resizes to the terminal `master`. Ends when
/// the stream does.
async fn read_client<R>(mut rd: R, feed: mpsc::Sender<Vec<u8>>, master: Option<OwnedFd>)
where
    R: AsyncRead + Unpin,
{
    let (mut buf, mut feed) = (Vec::new(), Some(feed));
    while let Ok(Some(frame)) = chan::read_frame(&mut rd, &mut buf).await {
        match frame {
            Frame::In(bytes) => {
                if let Some(tx) = &feed {
                    if tx.send(bytes.to_vec()).await.is_err() {
                        feed = None;
                    }
                }
            }
            Frame::InEnd => feed = None,
            Frame::Resize(size) => {
                if let Some(master) = &master {
                    let _ = pty::resize(master, size);
                }
            }
            Frame::Out(_) | Frame::Err(_) | Frame::Exit(_) => break,
        }
    }
}

/// Writes what arrives on `rx` to the command's stdin, and closes it when `rx` ends; a terminal
/// gets an end-of-transmission byte instead.
async fn feed(mut sink: Sink, mut rx: mpsc::Receiver<Vec<u8>>, eot: bool) {
    while let Some(bytes) = rx.recv().await {
        if sink.write_all(&bytes).await.is_err() || sink.flush().await.is_err() {
            return;
        }
    }
    if eot {
        let _ = sink.write_all(&[4]).await;
        let _ = sink.flush().await;
    }
    let _ = sink.shutdown().await;
}

/// Ends the carrier's process group: TERM, then KILL if it is still there after [`GRACE`].
async fn end_group(child: &mut Child) {
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw);
    let Some(group) = group else { return };
    let _ = kill_process_group(group, Signal::TERM);
    if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
        let _ = kill_process_group(group, Signal::KILL);
        let _ = child.wait().await;
    }
}

/// Starts `carrier` on `script` with its stdin and stdout piped, as the leader of a process
/// group, to carry a forwarded connection.
pub(super) fn start_forward(mut carrier: Command, script: &str) -> io::Result<Child> {
    carrier
        .arg(script)
        .process_group(0)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// Carries `stream`'s bytes to the forward `child` and its output back until the far side
/// closes, or the controller does and the far side has had [`GRACE`] to finish.
pub(super) async fn bridge<S>(mut child: Child, stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return;
    };
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (gone_tx, gone) = tokio::sync::oneshot::channel::<()>();
    let up = async {
        let _ = tokio::io::copy(&mut rd, &mut stdin).await;
        drop(stdin);
        let _ = gone_tx.send(());
    };
    let down = async {
        let far_closed = tokio::io::copy(&mut stdout, &mut wr);
        let controller_left = async {
            let _ = gone.await;
            tokio::time::sleep(GRACE).await;
        };
        tokio::select! { _ = far_closed => {}, () = controller_left => {} }
        let _ = wr.shutdown().await;
    };
    tokio::join!(up, down);
    end_group(&mut child).await;
}

/// Runs the probe `carrier` on `script` and says whether it exited 0.
pub(super) async fn probe(mut carrier: Command, script: &str) -> io::Result<bool> {
    let status = carrier
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    Ok(tokio::time::timeout(GRACE, status)
        .await
        .map_err(|_| io::Error::other("the probe took too long"))??
        .success())
}
