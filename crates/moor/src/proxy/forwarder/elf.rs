//! Whether an executable needs a dynamic loader, read from its ELF program headers. Neither
//! `object` nor `goblin` is a dependency, and one header type is all this reads.

use std::path::Path;

/// The program header type that names the loader.
const PT_INTERP: usize = 3;

/// The loader the ELF executable `bytes` names, such as `/lib64/ld-linux-x86-64.so.2`; none for a
/// static one, a static PIE included, and for anything that is not ELF.
pub(crate) fn interpreter(bytes: &[u8]) -> Option<String> {
    if bytes.get(..4)? != b"\x7fELF" {
        return None;
    }
    let wide = match bytes.get(4)? {
        1 => false,
        2 => true,
        _ => return None,
    };
    let big = *bytes.get(5)? == 2;
    let int = |at: usize, len: usize| -> Option<usize> {
        let raw = bytes.get(at..at.checked_add(len)?)?;
        let push = |n: u64, b: &u8| n << 8 | u64::from(*b);
        let n = if big {
            raw.iter().fold(0, push)
        } else {
            raw.iter().rev().fold(0, push)
        };
        usize::try_from(n).ok()
    };
    let (phoff, size, count, word) = if wide {
        (int(0x20, 8)?, int(0x36, 2)?, int(0x38, 2)?, 8)
    } else {
        (int(0x1c, 4)?, int(0x2a, 2)?, int(0x2c, 2)?, 4)
    };
    let (offset_at, filesz_at) = if wide { (8, 32) } else { (4, 16) };
    (0..count).find_map(|i| {
        let at = phoff.checked_add(i.checked_mul(size)?)?;
        if int(at, 4)? != PT_INTERP {
            return None;
        }
        let start = int(at.checked_add(offset_at)?, word)?;
        let len = int(at.checked_add(filesz_at)?, word)?;
        let raw = bytes.get(start..start.checked_add(len)?)?;
        Some(
            String::from_utf8_lossy(raw)
                .trim_end_matches('\0')
                .to_owned(),
        )
    })
}

/// Why the forwarder at `bin`, which needs `loader`, cannot run in a Box.
pub(crate) fn dynamic(bin: &Path, loader: &str) -> String {
    format!(
        "moor-egress-forward at {} is dynamically linked (it needs {loader}), so it cannot run in a Box",
        bin.display()
    )
}

/// What to install instead of a dynamically linked forwarder.
pub(crate) const STATIC: &str = "install the static build install.sh ships, or build it with \
    `cargo build -p moor-egress-forward --target <arch>-unknown-linux-musl`";

#[cfg(test)]
pub(crate) mod tests;
