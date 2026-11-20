//! The marker that says a private cache copy finished.

/// The marker volume that says the copy into `private` finished.
pub(super) fn marker(private: &str) -> String {
    format!("{private}.ok")
}

/// What to do with a private volume, by whether it and its marker exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Private {
    /// A finished copy; use it as it is.
    Trust,
    /// None yet; make it.
    Make,
    /// A copy a crash cut short; remove it and make it again.
    Redo,
}

/// How a private volume stands, from whether it and its marker exist.
pub(super) const fn standing(volume: bool, marked: bool) -> Private {
    match (volume, marked) {
        (true, true) => Private::Trust,
        (false, _) => Private::Make,
        (true, false) => Private::Redo,
    }
}
