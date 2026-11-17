//! The network Boxes join. Its MTU is the host's route's, because a tailscale route is 1280 and
//! larger packets vanish. The work place's init gives its own Docker the MTU of `eth0`, so the
//! network's is the one number to get right.

use tokio::process::Command;

use super::cli::Cli;
use crate::boxes::provider::{Notes, ProvideError};

/// Hosts whose route a Box's traffic takes: GitHub, then two public resolvers.
const PROBES: [&str; 3] = ["140.82.112.6", "1.1.1.1", "8.8.8.8"];
/// The MTU of an ordinary network.
const ETHERNET: u32 = 1500;
/// The driver option that sets a bridge network's MTU.
const MTU_OPTION: &str = "com.docker.network.driver.mtu";

/// The device an `ip route get` answer sends through.
pub(super) fn device_of(route: &str) -> Option<&str> {
    let mut words = route.split_whitespace();
    words.find(|w| *w == "dev")?;
    words.next()
}

/// The MTU of the route to `host`, when this machine can tell.
async fn route_mtu(host: &str) -> Option<u32> {
    let out = Command::new("ip")
        .args(["route", "get", host])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let dev = device_of(&text)?;
    tokio::fs::read_to_string(format!("/sys/class/net/{dev}/mtu"))
        .await
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The smallest MTU on the routes Boxes use.
pub(super) async fn probe() -> u32 {
    let mut mtu = ETHERNET;
    for host in PROBES {
        if let Some(m) = route_mtu(host).await {
            mtu = mtu.min(m);
        }
    }
    mtu
}

/// Makes sure network `name` exists with an MTU no larger than the host's route carries, and
/// returns that MTU. A network that is there already is reused, never replaced, because other
/// Boxes may be on it.
pub(super) async fn ensure(name: &str, note: &Notes) -> Result<u32, ProvideError> {
    let want = probe().await;
    let have = Cli::output([
        "network",
        "inspect",
        "-f",
        &format!("{{{{index .Options \"{MTU_OPTION}\"}}}}"),
        name,
    ])
    .await?;
    if have.status.success() {
        let have: u32 = String::from_utf8_lossy(&have.stdout)
            .trim()
            .parse()
            .unwrap_or(ETHERNET);
        return if have <= want {
            Ok(have)
        } else {
            Err(ProvideError::Failed(format!(
                "Docker network {name} has MTU {have} but this host's route carries {want}; \
                 remove the network when no Box is on it"
            )))
        };
    }
    note.say(&format!("creating network {name} with MTU {want}"));
    let made = Cli::ok([
        "network",
        "create",
        "--opt",
        &format!("{MTU_OPTION}={want}"),
        name,
    ])
    .await;
    // Another open may have made it first.
    if made.is_err() && !Cli::passes(["network", "inspect", name]).await {
        made?;
    }
    Ok(want)
}
