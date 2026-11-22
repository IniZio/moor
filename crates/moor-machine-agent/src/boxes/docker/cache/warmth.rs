//! How many of a Box's cache volumes hold data from an earlier Box.

use super::{Cli, Mount};

/// Prints `full` once for each directory under `/v` that holds anything.
const HOLDING: &str = "for d in /v/*; do [ -z \"$(ls -A \"$d\")\" ] || echo full; done";

/// The note that says how many of `mounts` hold data from an earlier Box, read in one throwaway
/// container; a volume that exists but holds nothing is cold.
pub(super) async fn warmth(image: &str, mounts: &[Mount]) -> String {
    let mut a = ["run", "--rm", "-u", "0", "--entrypoint", "sh"]
        .map(str::to_owned)
        .to_vec();
    for (i, m) in mounts.iter().enumerate() {
        a.push("-v".to_owned());
        a.push(format!("{}:/v/{i}:ro", m.volume()));
    }
    a.extend([image.to_owned(), "-c".to_owned(), HOLDING.to_owned()]);
    let all = mounts.len();
    match Cli::ok(a).await {
        Ok(said) => {
            let warm = said.lines().filter(|l| *l == "full").count();
            format!(
                "cache volumes: {warm} warm, {} cold",
                all.saturating_sub(warm)
            )
        }
        Err(e) => format!("cache volumes: {all} mounted, not read: {e}"),
    }
}
