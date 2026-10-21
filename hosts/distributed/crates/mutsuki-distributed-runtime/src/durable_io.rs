//! Crash-safe file replacement shared by the distributed Host's durable state
//! (registry snapshots, HA control log, resource catalog, content metadata).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Replaces `path` with `bytes` through a sibling temporary and a rename, so a
/// crash leaves either the old or the new complete file.
///
/// With `durable`, the temporary is fsynced before the rename and, on Unix, the
/// parent directory is fsynced after it so the rename itself survives power
/// loss. Windows has no supported directory flush through std; there the
/// synced file plus the atomic rename is the strongest available guarantee.
pub(crate) fn replace_file(path: &Path, bytes: &[u8], durable: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    if durable {
        file.sync_all()?;
    }
    drop(file);
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if durable && let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
