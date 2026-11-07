//! The warmups and forks going on, each in the background: a request that started one can be
//! cut off without ending it, and a later request can wait for how it ends.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::watch;

/// How a boot ended; none while it goes on.
type Ending = Option<Result<(), String>>;

/// The boots going on, by the lease's dir.
static FLIGHTS: Mutex<BTreeMap<PathBuf, watch::Receiver<Ending>>> = Mutex::new(BTreeMap::new());

/// The boots going on, whichever thread poisoned them.
fn flights() -> MutexGuard<'static, BTreeMap<PathBuf, watch::Receiver<Ending>>> {
    FLIGHTS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lets the next boot of its lease start once this boot ends, however it ends.
struct Landed(PathBuf);

impl Drop for Landed {
    fn drop(&mut self) {
        flights().remove(&self.0);
    }
}

/// Runs `boot` in the background, which outlives the caller, and waits for it. A boot of `dir`
/// already going on is refused, as crabbox refuses a slug in use.
pub(super) async fn fly(
    dir: &Path,
    boot: impl Future<Output = Result<(), String>> + Send + 'static,
) -> Result<(), String> {
    let (tell, ending) = watch::channel(None);
    {
        let mut going = flights();
        if going.contains_key(dir) {
            return Err(format!("{} is still booting", dir.display()));
        }
        going.insert(dir.to_path_buf(), ending.clone());
    }
    let landed = Landed(dir.to_path_buf());
    tokio::spawn(async move {
        let _landed = landed;
        let ended = boot.await;
        let _ = tell.send(Some(ended));
    });
    landing(ending).await
}

/// Waits for the boot of `dir` going on, and says how it ended; none when no boot is going on.
pub(super) async fn join(dir: &Path) -> Option<Result<(), String>> {
    let ending = flights().get(dir).cloned()?;
    Some(landing(ending).await)
}

/// How the boot behind `ending` ended.
async fn landing(mut ending: watch::Receiver<Ending>) -> Result<(), String> {
    ending.wait_for(Option::is_some).await.map_or_else(
        |_| Err("the boot ended without saying how".to_owned()),
        |ended| ended.clone().unwrap_or(Ok(())),
    )
}
