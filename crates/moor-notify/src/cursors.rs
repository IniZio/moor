//! The last event each hook is done with, kept in the state dir so a restart neither posts
//! an event twice nor skips one that came while the front was down.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// The file's name inside the state dir.
const FILE: &str = "cursors.json";

/// Each hook's cursor by its key, mirrored to `<state dir>/cursors.json` when there is a
/// state dir.
#[derive(Debug)]
pub struct Cursors {
    /// The file, when the front has a state dir.
    path: Option<PathBuf>,
    /// The cursors as last written.
    map: Mutex<BTreeMap<String, i64>>,
}

impl Cursors {
    /// The cursors saved under `dir`, or none kept anywhere but memory without one.
    ///
    /// # Errors
    /// The file exists but cannot be read or is not a JSON object of cursors.
    pub fn open(dir: Option<PathBuf>) -> Result<Self, String> {
        let path = dir.map(|d| d.join(FILE));
        let map = match &path {
            None => BTreeMap::new(),
            Some(p) => match fs::read(p) {
                Ok(bytes) => {
                    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
                Err(e) => return Err(format!("{}: {e}", p.display())),
            },
        };
        Ok(Self {
            path,
            map: Mutex::new(map),
        })
    }

    /// The cursor saved for `key`.
    pub fn get(&self, key: &str) -> Option<i64> {
        self.lock().get(key).copied()
    }

    /// Saves `cursor` for `key`, replacing the file in one rename.
    ///
    /// # Errors
    /// The file could not be written; the cursor is still kept in memory.
    pub fn set(&self, key: &str, cursor: i64) -> Result<(), String> {
        let bytes = {
            let mut map = self.lock();
            map.insert(key.to_owned(), cursor);
            serde_json::to_vec(&*map)
        };
        let Some(path) = &self.path else {
            return Ok(());
        };
        let tmp = path.with_extension("json.tmp");
        let write = || -> io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&bytes.map_err(io::Error::other)?)?;
            file.sync_all()?;
            fs::rename(&tmp, path)
        };
        write().map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Locks the map, through a poisoned lock too.
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, i64>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::Cursors;

    /// A saved cursor is read back by the next process.
    #[test]
    fn survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = Cursors::open(Some(dir.path().to_owned())).unwrap();
        assert_eq!(first.get("a@example.com"), None);
        first.set("a@example.com", 42).unwrap();
        let second = Cursors::open(Some(dir.path().to_owned())).unwrap();
        assert_eq!(second.get("a@example.com"), Some(42));
    }

    /// Without a state dir the cursor lives in memory only.
    #[test]
    fn memory_only_without_a_dir() {
        let cursors = Cursors::open(None).unwrap();
        cursors.set("a@example.com", 7).unwrap();
        assert_eq!(cursors.get("a@example.com"), Some(7));
    }
}
