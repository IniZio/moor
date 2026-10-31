//! Reaching a lease's VM: the ssh words crabbox prints, resolved by `ssh -G` on this host and
//! held to the host-key pin rules. Only this host runs them; the runner gets data and a pipe.

use std::fs::OpenOptions;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::Stdio;

use moor_client::link::lease::Answer;
use moor_client::link::ops::{self, Reply};
use moor_client::link::Channel;
use moor_core::boxscript::quote;
use moor_core::ssh::pin::{self, KnownHosts};
use moor_core::ssh::split_words;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

use super::{Crabbox, CALL};

/// Where a lease's sshd is, as ssh resolves its words.
struct Route {
    /// The login user.
    user: String,
    /// The name its host key is filed under: the alias, or the host name.
    alias: String,
    /// The host ssh connects to.
    hostname: String,
    /// Its port.
    port: String,
    /// The command whose stdio reaches the sshd, if any.
    proxy: Option<String>,
    /// The known-hosts files, in ssh's order.
    files: Vec<String>,
    /// crabbox's words, which pass the pin rules.
    words: Vec<String>,
}

/// The lease's login user and host keys.
pub(super) async fn info(lease: &Crabbox<'_>) -> Result<Answer, String> {
    let route = route(lease).await?;
    let mut names = vec![route.alias.clone()];
    if route.port != "22" {
        names.push(format!("[{}]:{}", route.alias, route.port));
    }
    let mut host_keys: Vec<String> = Vec::new();
    for file in &route.files {
        for name in &names {
            let out = Command::new("ssh-keygen")
                .args(["-F", name, "-f", file])
                .env("HOME", lease.home)
                .output()
                .await
                .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
            let text = String::from_utf8_lossy(&out.stdout);
            let keys = text
                .lines()
                .filter(|l| !l.starts_with(['#', '@']))
                .filter_map(
                    |l| match l.split_whitespace().collect::<Vec<_>>().as_slice() {
                        [_, kind, key, ..] => Some(format!("{kind} {key}")),
                        _ => None,
                    },
                );
            for key in keys {
                if !host_keys.contains(&key) {
                    host_keys.push(key);
                }
            }
        }
    }
    if host_keys.is_empty() {
        return Err(format!("no host key is known for {} yet", route.alias));
    }
    Ok(Answer::Info {
        user: route.user,
        host_keys,
    })
}

/// Appends OpenSSH public `key` to the VM's `authorized_keys` through crabbox's own words, once.
/// The key is parsed and printed afresh, so no text of the request reaches the VM's shell.
pub(super) async fn authorize(lease: &Crabbox<'_>, key: &str) -> Result<Answer, String> {
    let key = ssh_key::PublicKey::from_openssh(key)
        .and_then(|k| k.to_openssh())
        .map_err(|e| format!("not an OpenSSH public key: {e}"))?;
    let route = route(lease).await?;
    let k = quote(&key);
    let script = format!(
        "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && \
         {{ grep -qxF {k} ~/.ssh/authorized_keys || printf '%s\\n' {k} >> ~/.ssh/authorized_keys; }}"
    );
    let ran = Command::new("ssh")
        .args(&route.words)
        .arg(script)
        .env("HOME", lease.home)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(CALL, ran)
        .await
        .map_err(|_| "the key install ran too long".to_owned())?
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    if !out.status.success() {
        return Err(format!("the key install failed ({:?})", out.status.code()));
    }
    Ok(Answer::Done)
}

/// A byte pipe to a lease's sshd.
pub enum Sshd {
    /// The words' proxy command, running.
    Proxy(Child),
    /// A plain connection.
    Tcp(TcpStream),
}

/// Opens a pipe to the sshd of lease `slug` under `home`.
pub async fn dial(home: &Path, pool: &super::Pool, slug: &str) -> Result<Sshd, String> {
    let id = super::checked(slug)?;
    let route = route(&Crabbox {
        home,
        pool,
        id: Some(id),
    })
    .await?;
    let Some(proxy) = route.proxy.as_deref() else {
        let addr = format!("{}:{}", route.hostname, route.port);
        return TcpStream::connect(&addr)
            .await
            .map(Sshd::Tcp)
            .map_err(|e| format!("cannot reach {addr}: {e}"));
    };
    Command::new("/bin/sh")
        .args(["-c", &expand(proxy, &route)])
        .current_dir(home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map(Sshd::Proxy)
        .map_err(|e| format!("cannot start the proxy command: {e}"))
}

/// Tells the runner the dial reached the VM, then carries its bytes.
pub async fn carry(mut ch: Channel, sshd: Sshd) {
    if ops::write_line(&mut ch, &Reply::Started).await.is_ok() {
        splice(sshd, ch).await;
    }
}

/// Carries bytes between `stream` and `sshd` until both directions end.
pub async fn splice<S: AsyncRead + AsyncWrite + Unpin>(sshd: Sshd, mut stream: S) {
    match sshd {
        Sshd::Tcp(mut tcp) => {
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
        }
        Sshd::Proxy(mut child) => {
            if let (Some(mut out), Some(mut input)) = (child.stdout.take(), child.stdin.take()) {
                let (mut rd, mut wr) = tokio::io::split(stream);
                let to = async move {
                    let _ = tokio::io::copy(&mut rd, &mut input).await;
                };
                let from = async move {
                    let _ = tokio::io::copy(&mut out, &mut wr).await;
                    let _ = wr.shutdown().await;
                };
                tokio::join!(to, from);
            }
            let _ = child.kill().await;
        }
    }
}

/// The lease's words from `crabbox ssh`, resolved by `ssh -G`, once they pin the host key.
async fn route(lease: &Crabbox<'_>) -> Result<Route, String> {
    let printed = lease.run("ssh", "--id", CALL, &[0]).await?;
    let words = printed
        .lines()
        .filter_map(split_words)
        .find_map(|w| {
            (w.first().map(String::as_str) == Some("ssh"))
                .then(|| w.get(1..).unwrap_or_default().to_vec())
        })
        .ok_or("crabbox ssh printed no ssh line")?;
    let out = Command::new("ssh")
        .arg("-G")
        .args(&words)
        .env("HOME", lease.home)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("cannot run ssh -G: {e}"))?;
    if !out.status.success() {
        return Err("ssh -G could not read crabbox's words".to_owned());
    }
    let options = String::from_utf8_lossy(&out.stdout);
    let home = lease.home.to_string_lossy();
    let files: Vec<String> = pin::known_hosts_files(&options)
        .into_iter()
        .map(|f| {
            f.strip_prefix("~/")
                .map_or_else(|| f.to_owned(), |rest| format!("{home}/{rest}"))
        })
        .collect();
    pin::check(&options, &Local(&files)).map_err(|why| pin::refusal(&why))?;
    let get = |key: &str| {
        options
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(' '))
            .map(str::to_owned)
    };
    let hostname = get("hostname").ok_or("ssh -G named no host")?;
    Ok(Route {
        user: get("user").ok_or("ssh -G named no user")?,
        alias: get("hostkeyalias").unwrap_or_else(|| hostname.clone()),
        port: get("port").unwrap_or_else(|| "22".to_owned()),
        proxy: get("proxycommand").filter(|p| p != "none"),
        hostname,
        files,
        words,
    })
}

/// `cmd` with ssh's `%h`, `%p`, `%r`, `%n` and `%%` filled in from `route`.
fn expand(cmd: &str, route: &Route) -> String {
    let mut out = String::new();
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(&route.hostname),
            Some('p') => out.push_str(&route.port),
            Some('r') => out.push_str(&route.user),
            Some('n') => out.push_str(&route.alias),
            Some('%') | None => out.push('%'),
            Some(other) => out.extend(['%', other]),
        }
    }
    out
}

/// The known-hosts files on this host, with `~` filled in.
struct Local<'a>(&'a [String]);

impl Local<'_> {
    /// `file` as listed, or with `~` filled in.
    fn path<'f>(&'f self, file: &'f str) -> &'f str {
        self.0
            .iter()
            .find(|f| f.ends_with(file.trim_start_matches('~')))
            .map_or(file, String::as_str)
    }
}

impl KnownHosts for Local<'_> {
    fn is_device(&self, file: &str) -> bool {
        std::fs::metadata(self.path(file))
            .is_ok_and(|m| m.file_type().is_char_device() || m.file_type().is_block_device())
    }

    fn can_add_to(&self, file: &str) -> bool {
        let path = Path::new(self.path(file));
        if path.exists() {
            return OpenOptions::new().append(true).open(path).is_ok();
        }
        path.parent()
            .and_then(|dir| std::fs::metadata(dir).ok())
            .is_some_and(|m| m.is_dir() && !m.permissions().readonly())
    }
}
