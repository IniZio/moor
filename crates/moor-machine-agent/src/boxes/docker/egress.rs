//! The hosts a Docker Box reached, kept in its run volume for the controller to read.
//!
//! Two sources feed one tally per Box. tinyproxy in the agent place logs each request it takes,
//! naming the host and port; a host it sends to the Proxy is `credential`, any other `proxied`.
//! An nftables table in the Box's network namespace puts each new connection that skips
//! tinyproxy in a set, keyed by address and port with a counter; those are `direct`. A helper
//! container with `NET_ADMIN` in that namespace and nothing else installs the table and lists
//! the sets, since the agent place is not privileged. Nothing here ever blocks a connection.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moor_core::domain::egress::{EgressPath, EgressRow, EgressTally, SEEN_FILE};
use moor_core::exec_env::PROXY_LOG;
use serde_json::Value;
use tokio::time::Instant;

use super::cli::Cli;
use super::names::Names;
use crate::boxes::Slug;

/// Pause between reads of one Box.
const READ_EVERY: Duration = Duration::from_secs(30);
/// Past this many bytes read, the agent place's request log is emptied.
const LOG_KEEP: u64 = 1024 * 1024;
/// The nftables table that records direct connections.
const TABLE: &str = "inet moor_egress";
/// Separates the parts of the agent place's answer.
const SEP: char = '\u{1e}';

/// What one Box's tally holds between reads.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// The rows so far.
    tally: EgressTally,
    /// Bytes of the request log already counted.
    log_at: u64,
    /// The counter of each direct address and port at the last read.
    kernel: HashMap<(String, u16), u64>,
    /// Whether the tally came from the Box's file and this machine agent has not read the
    /// sources since: a row already in the tally then adds nothing, since its counts are in.
    fresh: bool,
    /// When the next read is due; none before the first.
    next: Option<Instant>,
}

impl Watch {
    /// A watch that goes on from the rows the Box's file held.
    pub(super) fn resumed(rows: Vec<EgressRow>) -> Self {
        Self {
            tally: EgressTally::of(rows),
            fresh: true,
            ..Self::default()
        }
    }

    /// Counts each request in the complete lines of `chunk`, the log read from where the last
    /// read stopped, against tinyproxy's config `conf`. Returns the bytes it counted.
    pub(super) fn take_log(&mut self, conf: &str, chunk: &str, at: u64) -> usize {
        let (whole, _) = chunk.rsplit_once('\n').unwrap_or_default();
        let used = if chunk.contains('\n') {
            whole.len() + 1
        } else {
            0
        };
        let upstreams = upstreams(conf);
        for (host, port) in whole.lines().filter_map(request) {
            let path = if upstreams.iter().any(|u| matches_host(u, &host)) {
                EgressPath::Credential
            } else {
                EgressPath::Proxied
            };
            self.add(&host, port, path, at, 1);
        }
        self.log_at += used as u64;
        used
    }

    /// Counts the direct connections the sets list, `(address, port, counter)`. A counter
    /// below the last one belongs to a new network namespace, so all of it is new.
    pub(super) fn take_sets(&mut self, sets: &[(String, u16, u64)], at: u64) {
        for (addr, port, n) in sets {
            let last = self.kernel.insert((addr.clone(), *port), *n);
            let new = match last {
                Some(last) if *n >= last => n - last,
                _ => *n,
            };
            self.add(addr, *port, EgressPath::Direct, at, new);
        }
    }

    /// The rows so far.
    pub(super) fn rows(&self) -> Vec<EgressRow> {
        self.tally.rows()
    }

    /// Adds `count`, unless a resumed tally already holds the row.
    fn add(&mut self, host: &str, port: u16, path: EgressPath, at: u64, count: u64) {
        if !(self.fresh && self.tally.contains(host, port, path)) {
            self.tally.add(host, port, path, at, count);
        }
    }
}

/// The host and port of a tinyproxy `Request` log line; none for any other line. Only the
/// authority is read, never the path, which may carry a secret.
pub(super) fn request(line: &str) -> Option<(String, u16)> {
    let (_, rest) = line.split_once("Request (")?;
    let (_, rest) = rest.split_once("): ")?;
    let mut words = rest.split_whitespace();
    let method = words.next()?;
    let target = words.next()?;
    let (authority, default) = if method == "CONNECT" {
        (target, None)
    } else {
        let rest = target.strip_prefix("http://")?;
        (rest.split('/').next()?, Some(80))
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
        _ => (authority, default?),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port))
}

/// The hosts tinyproxy's config `conf` sends to the Proxy.
pub(super) fn upstreams(conf: &str) -> Vec<String> {
    conf.lines()
        .filter(|l| l.starts_with("upstream "))
        .filter_map(|l| {
            let quoted = l.trim_end().strip_suffix('"')?;
            let (_, host) = quoted.rsplit_once('"')?;
            Some(host.to_ascii_lowercase())
        })
        .collect()
}

/// Whether tinyproxy's `upstream` host `pattern` covers `host`: itself, or with a leading `.`
/// any name under it.
fn matches_host(pattern: &str, host: &str) -> bool {
    if pattern.starts_with('.') {
        host.ends_with(pattern)
    } else {
        host == pattern
    }
}

/// The `(address, port, counter)` of each element in `nft -j list set` output, one JSON
/// document per line.
pub(super) fn set_elements(listed: &str) -> Vec<(String, u16, u64)> {
    let mut out = Vec::new();
    for doc in listed
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let items = doc.pointer("/nftables").and_then(Value::as_array);
        let sets = items.into_iter().flatten();
        let elems = sets.filter_map(|i| i.pointer("/set/elem").and_then(Value::as_array));
        out.extend(elems.flatten().filter_map(element));
    }
    out
}

/// The `(address, port, counter)` of one set element.
fn element(elem: &Value) -> Option<(String, u16, u64)> {
    let addr = elem.pointer("/elem/val/concat/0")?.as_str()?;
    let port = u16::try_from(elem.pointer("/elem/val/concat/1")?.as_u64()?).ok()?;
    let count = elem
        .pointer("/elem/counter/packets")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    Some((addr.to_owned(), port, count))
}

/// The `sh` the helper runs in the Box's network namespace: makes the table when it is missing,
/// then lists its sets. The output hook skips loopback and every socket of the agent place's
/// cgroup, which is where tinyproxy and `moor-egress-forward` dial from; the forward hook sees
/// the containers the work place's Docker runs. The helper shares the agent place's pid and the
/// host's cgroup namespace, so `/proc/1/cgroup` names the agent place's cgroup in full.
pub(super) fn helper_script() -> String {
    let mut record = String::new();
    for (family, addr) in [("4", "ip"), ("6", "ip6")] {
        for proto in ["tcp", "udp"] {
            let _ = writeln!(
                record,
                "    meta nfproto ipv{family} meta l4proto {proto} add @seen{family} {{ {addr} daddr . th dport }}"
            );
        }
    }
    format!(
        r#"set -e
if ! nft list table {TABLE} >/dev/null 2>&1; then
  cg=$(sed -n 's|^0::/||p' /proc/1/cgroup)
  skip=
  if [ -n "$cg" ]; then skip="socket cgroupv2 level $(printf '%s' "$cg" | awk -F/ '{{print NF}}') \"$cg\" return"; fi
  nft -f - <<EOF
table {TABLE} {{
  set seen4 {{ type ipv4_addr . inet_service; flags dynamic; size 4096; counter; }}
  set seen6 {{ type ipv6_addr . inet_service; flags dynamic; size 4096; counter; }}
  chain record {{
{record}  }}
  chain local {{
    type filter hook output priority 0; policy accept;
    ct state != new return
    oifname "lo" return
    $skip
    jump record
  }}
  chain forwarded {{
    type filter hook forward priority 0; policy accept;
    ct state new oifname != "lo" jump record
  }}
}}
EOF
fi
nft -j list set {TABLE} seen4
nft -j list set {TABLE} seen6
"#
    )
}

/// Runs the helper against the Box `names` from `image`: the table is in place afterwards, and
/// its sets' elements are returned.
pub(super) async fn read_sets(
    image: &str,
    names: &Names,
) -> Result<Vec<(String, u16, u64)>, String> {
    let netns = format!("container:{}", names.agent);
    let script = helper_script();
    let args = [
        "run",
        "--rm",
        "--network",
        &netns,
        "--pid",
        &netns,
        "--cgroupns",
        "host",
        "--cap-drop",
        "ALL",
        "--cap-add",
        "NET_ADMIN",
        "--user",
        "0",
        "--entrypoint",
        "sh",
        image,
        "-c",
        &script,
    ];
    Cli::ok(args)
        .await
        .map(|listed| set_elements(&listed))
        .map_err(|e| e.to_string())
}

/// The agent place's answer to [`read_log`]: the log's size, tinyproxy's config, and the log
/// from byte `from` on.
fn log_script(from: u64) -> String {
    let log = format!("$HOME/{PROXY_LOG}");
    format!(
        "wc -c < \"{log}\" 2>/dev/null || echo 0; printf '\\036'; cat \"$HOME/.moor/egress.conf\" 2>/dev/null; printf '\\036'; tail -c +{} \"{log}\" 2>/dev/null; true",
        from + 1
    )
}

/// Reads the agent place's request log of the Box `names` into `watch`, emptying the log once
/// [`LOG_KEEP`] bytes of it were counted.
async fn read_log(watch: &mut Watch, names: &Names, at: u64) -> Result<(), String> {
    let script = log_script(watch.log_at);
    let out = Cli::output(["exec", &names.agent, "sh", "-c", &script])
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.splitn(3, SEP);
    let size: u64 = parts.next().unwrap_or("").trim().parse().unwrap_or(0);
    let conf = parts.next().unwrap_or("");
    let chunk = parts.next().unwrap_or("");
    if size < watch.log_at {
        watch.log_at = 0;
        return Ok(());
    }
    watch.take_log(conf, chunk, at);
    if watch.log_at >= LOG_KEEP {
        let empty = format!(": > \"$HOME/{PROXY_LOG}\"");
        Cli::ok(["exec", &names.agent, "sh", "-c", &empty])
            .await
            .map_err(|e| e.to_string())?;
        watch.log_at = 0;
    }
    Ok(())
}

/// Every Box's watch, each with at most one read running.
#[derive(Default)]
pub(super) struct Watches(Mutex<BTreeMap<Slug, Arc<tokio::sync::Mutex<Option<Watch>>>>>);

impl Watches {
    /// Reads the Box `slug` in the background from the agent place's `image`, when its read is
    /// due and no read of it runs.
    pub(super) fn read_when_due(&self, slug: &Slug, image: &str) {
        let watch = {
            let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(all.entry(slug.clone()).or_default())
        };
        let Ok(mut guard) = watch.try_lock_owned() else {
            return;
        };
        if guard
            .as_ref()
            .and_then(|w| w.next)
            .is_some_and(|next| Instant::now() < next)
        {
            return;
        }
        let names = Names::of(slug);
        let image = image.to_owned();
        tokio::spawn(async move {
            let w = match guard.take() {
                Some(w) => w,
                None => resume(&names).await,
            };
            *guard = Some(read(w, &names, &image).await);
        });
    }

    /// Forgets the Boxes not in `live`.
    pub(super) fn keep_only(&self, live: &[Slug]) {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|slug, _| live.contains(slug));
    }
}

/// The watch of a Box this machine agent has not read yet, from the rows its file holds.
async fn resume(names: &Names) -> Watch {
    let held = Cli::ok(["exec", &names.agent, "cat", SEEN_FILE]).await;
    let rows = held
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Watch::resumed(rows)
}

/// One read of both sources into `w`, and the Box's file rewritten when a row changed. A source
/// that cannot be read is skipped until the next read.
async fn read(mut w: Watch, names: &Names, image: &str) -> Watch {
    let at = now_ms();
    let before = w.rows();
    // Neither failure is the Box's: an older agent image has no nft, and a stopped agent place
    // answers nothing. The next read tries again.
    drop(read_log(&mut w, names, at).await);
    if let Ok(sets) = read_sets(image, names).await {
        w.take_sets(&sets, at);
    }
    w.fresh = false;
    w.next = Some(Instant::now() + READ_EVERY);
    let rows = w.rows();
    if rows != before {
        if let Ok(json) = serde_json::to_string(&rows) {
            let script = format!("cat > {SEEN_FILE}.new && mv -f {SEEN_FILE}.new {SEEN_FILE}");
            drop(Cli::feed(["exec", "-i", &names.agent, "sh", "-c", &script], &json).await);
        }
    }
    w
}

/// Now, in Unix milliseconds.
#[expect(
    clippy::disallowed_methods,
    reason = "a row's first_seen is wall-clock time"
)]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
