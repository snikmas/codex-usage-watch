//! Private filesystem primitives for tracker-owned state.
//!
//! Unix permission bits are not a Windows security boundary, so the Unix mode
//! enforcement is deliberately compiled separately from the portable atomic
//! write behavior.

use std::fs::{self, DirBuilder};
use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

pub const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Create or repair only the tracker-owned directory. Its parent is never
/// chmodded, even when the parent was selected by the user.
pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        builder.mode(PRIVATE_DIRECTORY_MODE);
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))?;
    Ok(())
}

pub fn ensure_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn ensure_private_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => ensure_private_file(path),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Atomically replace a tracker-owned file. `NamedTempFile` creates the
/// temporary file privately; we still enforce the final mode explicitly so
/// existing permissive files are repaired on replacement.
pub fn write_private_atomic(directory: &Path, destination: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = NamedTempFile::new_in(directory)?;
    ensure_private_file(temporary.path())?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    ensure_private_file(destination)?;
    if let Ok(directory_file) = fs::File::open(directory) {
        let _ = directory_file.sync_all();
    }
    Ok(())
}
