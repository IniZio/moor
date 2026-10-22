//! Prelude lines that keep tinyproxy running on the Box loopback as the Box's egress proxy. It
//! sends each credential host to the Proxy tunnel and dials every other host direct.

use std::fmt::Write as _;

/// The start of the line, after `moor: `, that the prelude prints to stderr when it stops the exec
/// because the egress proxy could not be installed or started.
pub const EGRESS_FAILED: &str = "the egress proxy could not start";

/// Shell variables the egress lines set; the prelude unsets them at its end.
pub(super) const EGRESS_SHELL_VARS: &str =
    "moor_su moor_apt moor_conf moor_new moor_n moor_pid moor_ours moor_st";

/// The Box directory, under the Box user's home, that holds the files below; each is shell text
/// that expands on the Box.
const DIR: &str = "$HOME/.moor";
/// The tinyproxy config.
const CONF: &str = "$HOME/.moor/egress.conf";
/// The pid file tinyproxy writes.
const PID: &str = "$HOME/.moor/egress.pid";
/// The file whose `flock` lets one command at a time install, configure and start tinyproxy.
const LOCK: &str = "$HOME/.moor/egress.lock";
/// The apt output of the last install.
const INSTALL_LOG: &str = "$HOME/.moor/egress-install.log";
/// What tinyproxy printed when it last started.
const START_LOG: &str = "$HOME/.moor/egress-start.log";

/// Proxy variables apt must not see, so the install goes direct.
const PROXY_VARS: [&str; 8] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "https_proxy",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// Polls of 0.1 s each while waiting for the port to open or close.
const POLLS: u32 = 50;

/// Seconds a command waits for another command's install or start to finish.
const LOCK_WAIT: u32 = 300;

/// The tinyproxy config without its `PidFile` line: listen on loopback `listen`, and send each of
/// `hosts` to the Proxy tunnel on loopback `upstream`.
///
/// A quoted host matches only itself; a host that starts with `.` matches its subdomains.
pub(super) fn config(listen: u16, upstream: u16, hosts: &[String]) -> String {
    let mut hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    hosts.sort_unstable();
    hosts.dedup();
    let mut out =
        format!("Port {listen}\nListen 127.0.0.1\nDisableViaHeader Yes\nLogLevel Critical\n");
    for host in hosts {
        let _ = writeln!(out, "upstream http 127.0.0.1:{upstream} \"{host}\"");
    }
    out
}

/// `reason`, shell text inside double quotes, on stderr after the failure prefix, and the end of
/// the exec.
fn stop(reason: &str) -> String {
    format!("{{ echo \"moor: {EGRESS_FAILED}: {reason}\" >&2; exit 1; }}")
}

/// Lines that install tinyproxy when it is missing, write its config, and start it when it is not
/// listening on `listen` or its config changed. Every failure stops the exec with the reason.
///
/// A changed config goes to a temp file and replaces `~/.moor/egress.conf` only once the new
/// tinyproxy listens, so a failed restart leaves the old config and the next command retries.
///
/// One command at a time does this, under `flock` on a file in `~/.moor`, so two commands that
/// start together never start two tinyproxies. The listener is ours only while the pid file names
/// a live `tinyproxy`; a port held by anything else stops the exec.
pub(super) fn lines(listen: u16, upstream: u16, hosts: &[String]) -> Vec<String> {
    let unset = PROXY_VARS.iter().fold(String::new(), |mut out, v| {
        let _ = write!(out, " -u {v}");
        out
    });
    let up = format!("grep -q ' 0100007F:{listen:04X} 00000000:0000 0A' /proc/net/tcp 2>/dev/null");
    let install = format!(
        "command -v tinyproxy >/dev/null 2>&1 || {{ command -v apt-get >/dev/null 2>&1 || {}; if [ \"$(id -u)\" = 0 ]; then moor_su=; else moor_su=\"sudo -n\"; fi; moor_apt=\"$moor_su env{unset} DEBIAN_FRONTEND=noninteractive apt-get -q -y -o DPkg::Lock::Timeout=300\"; {{ $moor_apt install tinyproxy-bin || {{ $moor_apt update && $moor_apt install tinyproxy-bin; }}; }} >\"{INSTALL_LOG}\" 2>&1 || {}; }}",
        stop("tinyproxy is not installed and the Box has no apt-get; moor installs it only with apt"),
        stop(&format!("apt-get could not install tinyproxy-bin: $(tail -n 1 \"{INSTALL_LOG}\")")),
    );
    let config = format!(
        "moor_conf={}\"PidFile \\\"{PID}\\\"\"; moor_new=; if ! printf '%s\\n' \"$moor_conf\" | cmp -s - \"{CONF}\" 2>/dev/null; then printf '%s\\n' \"$moor_conf\" >\"{CONF}.new\" || {{ rm -f \"{CONF}.new\"; {}; }}; moor_new=1; fi",
        crate::boxscript::quote(&config(listen, upstream, hosts)),
        stop(&format!("its config could not be written to {CONF}")),
    );
    let ours = format!(
        "moor_pid=$(cat \"{PID}\" 2>/dev/null); moor_ours=; [ -n \"$moor_pid\" ] && [ \"$(cat \"/proc/$moor_pid/comm\" 2>/dev/null)\" = tinyproxy ] && moor_ours=1"
    );
    // tinyproxy removes its pid file as it exits, so the new one starts only once the old one
    // is gone; a zombie counts as gone.
    let old_runs = "moor_st=$(sed -n 's/.*) \\(.\\) .*/\\1/p' \"/proc/$moor_pid/stat\" 2>/dev/null) && [ -n \"$moor_st\" ] && [ \"$moor_st\" != Z ]";
    let start = format!(
        "if {up} && [ -z \"$moor_ours\" ]; then {}; fi; if [ -n \"$moor_new\" ] || ! {up}; then if [ -n \"$moor_ours\" ]; then kill \"$moor_pid\" 2>/dev/null; moor_n=0; while {old_runs}; do moor_n=$((moor_n+1)); [ $moor_n -le {POLLS} ] || {}; sleep 0.1; done; fi; moor_n=0; while {up}; do moor_n=$((moor_n+1)); [ $moor_n -le {POLLS} ] || {}; sleep 0.1; done; if [ -n \"$moor_new\" ]; then moor_cfg=\"{CONF}.new\"; else moor_cfg=\"{CONF}\"; fi; tinyproxy -c \"$moor_cfg\" </dev/null >/dev/null 2>\"{START_LOG}\" 9>&- || {}; moor_n=0; until {up}; do moor_n=$((moor_n+1)); [ $moor_n -le {POLLS} ] || {}; sleep 0.1; done; if [ -n \"$moor_new\" ]; then mv -f \"{CONF}.new\" \"{CONF}\" || {}; fi; fi",
        stop(&format!("port {listen} is taken by another process")),
        stop("the old tinyproxy $moor_pid did not exit after TERM"),
        stop(&format!("port {listen} stays held after tinyproxy was stopped")),
        stop(&format!("tinyproxy exited: $(tail -n 1 \"{START_LOG}\")")),
        stop(&format!("tinyproxy is not listening on 127.0.0.1:{listen}")),
        stop(&format!("its config could not be moved to {CONF}")),
    );
    vec![
        format!(
            "mkdir -p \"{DIR}\" || {}",
            stop(&format!("{DIR} could not be created"))
        ),
        format!(
            "( flock -w {LOCK_WAIT} 9 || {}; {install}; {config}; {ours}; {start} ) 9>\"{LOCK}\" || exit 1",
            stop(&format!("another command held {LOCK} for {LOCK_WAIT} s")),
        ),
    ]
}
