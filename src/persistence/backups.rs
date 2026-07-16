use std::fs;
use std::io;
use std::path::Path;

use super::{StateError, StateStore};
use crate::private_fs::ensure_private_file;

impl StateStore {
    pub fn backup_database(&self, destination: &Path) -> Result<(), StateError> {
        if destination.exists() {
            return Err(StateError::Io {
                path: destination.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "backup destination already exists",
                ),
            });
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| StateError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.connection
            .pragma_query_value(None, "wal_checkpoint", |row| row.get::<_, i64>(0))?;
        self.connection.execute(
            "VACUUM main INTO ?1",
            [destination.to_string_lossy().as_ref()],
        )?;
        ensure_private_file(destination).map_err(|source| StateError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}
