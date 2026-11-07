//! The warmups and forks going on, each in the background: a request that started one can be
//! cut off without ending it, and a later request can wait for how it ends.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::watch;

/// How a boot ended; none while it goes on.
type Ending = Option<Result<(), String>>;

/// A boot going on: how it ended, and what asks it to stop.
#[derive(Clone)]
struct Flight {
    /// How it ended.
    ending: watch::Receiver<Ending>,
    /// Set to ask it to stop.
    stop: Arc<watch::Sender<bool>>,
}

/// The boots going on, by the lease's dir.
static FLIGHTS: Mutex<BTreeMap<PathBuf, Flight>> = Mutex::new(BTreeMap::new());

tokio::task_local! {
    /// What asks the boot this code runs for to stop.
    static STOP: watch::Receiver<bool>;
}

/// The boots going on, whichever thread poisoned them.
fn flights() -> MutexGuard<'static, BTreeMap<PathBuf, Flight>> {
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
    let (stop, stopping) = watch::channel(false);
    {
        let mut going = flights();
        if going.contains_key(dir) {
            return Err(format!("{} is still booting", dir.display()));
        }
        let stop = Arc::new(stop);
        let ending = ending.clone();
        going.insert(dir.to_path_buf(), Flight { ending, stop });
    }
    let (landed, busy) = (
        Landed(dir.to_path_buf()),
        crate::busy::busy(&format!("a boot of {}", dir.display())),
    );
    tokio::spawn(async move {
        let _landed = (landed, busy);
        let ended = STOP.scope(stopping, boot).await;
        let _ = tell.send(Some(ended));
    });
    landing(ending).await
}

/// Waits for the boot of `dir` going on, and says how it ended; none when no boot is going on.
pub(super) async fn join(dir: &Path) -> Option<Result<(), String>> {
    let ending = flights().get(dir).cloned()?.ending;
    Some(landing(ending).await)
}

/// Asks the boot of `dir` going on to stop and waits for it; none going on is fine.
pub(super) async fn stop(dir: &Path) {
    let Some(flight) = flights().get(dir).cloned() else {
        return;
    };
    let _ = flight.stop.send(true);
    let _ = landing(flight.ending).await;
}

/// Resolves when the boot this code runs for is asked to stop; never outside a boot.
pub(super) async fn stop_asked() {
    let Ok(mut stopping) = STOP.try_with(Clone::clone) else {
        return std::future::pending().await;
    };
    if stopping.wait_for(|asked| *asked).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// How the boot behind `ending` ended.
async fn landing(mut ending: watch::Receiver<Ending>) -> Result<(), String> {
    ending.wait_for(Option::is_some).await.map_or_else(
        |_| Err("the boot ended without saying how".to_owned()),
        |ended| ended.clone().unwrap_or(Ok(())),
    )
}
