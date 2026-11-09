//! SQLite persistence and platform secret protection for the host.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use thiserror::Error;

/// One ordered schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The SQLite `user_version` written after the migration commits.
    pub version: u32,
    /// SQL run in the migration transaction.
    pub sql: &'static str,
}

/// Errors returned while opening or migrating a database.
#[derive(Debug, Error)]
pub enum SqliteError {
    /// The database operation failed.
    #[error("sqlite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A filesystem operation failed.
    #[error("database backup failed: {0}")]
    Io(#[from] io::Error),
    /// The database was written by a newer application version.
    #[error("database version {actual} is newer than supported version {supported}")]
    FutureVersion {
        /// 数据库实际记录的版本。
        actual: u32,
        /// 当前迁移列表支持的最高版本。
        supported: u32,
    },
    /// The migration list is not strictly ordered or contains version zero.
    #[error("migrations must be strictly ordered and start at a non-zero version")]
    InvalidMigrations,
    /// SQLite did not enable foreign-key enforcement.
    #[error("SQLite foreign-key enforcement could not be enabled")]
    ForeignKeysDisabled,
    /// The on-disk user version is outside SQLite's supported integer range.
    #[error("invalid SQLite user_version {0}")]
    InvalidVersion(i64),
}

/// Opens `path`, enables foreign keys, and applies each pending migration.
///
/// A copy at `path` with a `.bak` extension is made before the first pending
/// migration. Each migration and its `user_version` update are one transaction.
///
/// # Errors
/// Returns [`SqliteError::InvalidMigrations`] for unordered or zero versions,
/// [`SqliteError::FutureVersion`] when the file is newer than `migrations`,
/// [`SqliteError::ForeignKeysDisabled`] when enforcement cannot be enabled,
/// and SQLite or backup I/O errors. A failed migration rolls back its own transaction.
pub fn open_and_migrate(
    path: impl AsRef<Path>,
    migrations: &[Migration],
) -> Result<Connection, SqliteError> {
    validate_migrations(migrations)?;
    let path = path.as_ref();
    let mut connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(SqliteError::ForeignKeysDisabled);
    }

    let current = user_version(&connection)?;
    let supported = migrations.last().map_or(0, |migration| migration.version);
    if current > supported {
        return Err(SqliteError::FutureVersion {
            actual: current,
            supported,
        });
    }
    if migrations
        .iter()
        .any(|migration| migration.version > current)
        && path.exists()
    {
        fs::copy(path, backup_path(path))?;
    }

    for migration in migrations
        .iter()
        .filter(|migration| migration.version > current)
    {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration.sql)?;
        transaction.execute_batch(&format!("PRAGMA user_version = {};", migration.version))?;
        transaction.commit()?;
    }
    Ok(connection)
}

fn validate_migrations(migrations: &[Migration]) -> Result<(), SqliteError> {
    if migrations.iter().any(|migration| migration.version == 0)
        || migrations
            .windows(2)
            .any(|pair| pair[0].version >= pair[1].version)
    {
        return Err(SqliteError::InvalidMigrations);
    }
    Ok(())
}

fn user_version(connection: &Connection) -> Result<u32, SqliteError> {
    let value: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    u32::try_from(value).map_err(|_| SqliteError::InvalidVersion(value))
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("bak")
}

/// Errors returned by OS-backed secret protection.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The current platform has no implementation.
    #[error("secret protection is unsupported on this platform")]
    Unsupported,
    /// Windows DPAPI returned a system error code.
    #[error("Windows DPAPI failed with error code {0}")]
    Dpapi(u32),
}

/// Seals secret bytes using the current user's OS protection.
///
/// # Errors
/// Returns [`SecretError::Unsupported`] off Windows and [`SecretError::Dpapi`] when DPAPI fails.
pub fn seal_secret(secret: &[u8]) -> Result<Vec<u8>, SecretError> {
    secret::seal(secret)
}

/// Opens bytes previously returned by [`seal_secret`].
///
/// # Errors
/// Returns [`SecretError::Unsupported`] off Windows and [`SecretError::Dpapi`] when the
/// data is corrupt or was sealed by another user or machine.
pub fn open_secret(sealed: &[u8]) -> Result<Vec<u8>, SecretError> {
    secret::open(sealed)
}

#[cfg(not(windows))]
mod secret {
    use super::SecretError;

    pub(super) fn seal(_: &[u8]) -> Result<Vec<u8>, SecretError> {
        Err(SecretError::Unsupported)
    }

    pub(super) fn open(_: &[u8]) -> Result<Vec<u8>, SecretError> {
        Err(SecretError::Unsupported)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod secret {
    use std::{ptr, slice};

    use super::SecretError;
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    pub(super) fn seal(input: &[u8]) -> Result<Vec<u8>, SecretError> {
        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: input.len().try_into().map_err(|_| SecretError::Dpapi(0))?,
            pbData: input.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input_blob,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(SecretError::Dpapi(unsafe { GetLastError() }));
        }
        copy_and_free(output)
    }

    pub(super) fn open(input: &[u8]) -> Result<Vec<u8>, SecretError> {
        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: input.len().try_into().map_err(|_| SecretError::Dpapi(0))?,
            pbData: input.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input_blob,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(SecretError::Dpapi(unsafe { GetLastError() }));
        }
        copy_and_free(output)
    }

    fn copy_and_free(blob: CRYPT_INTEGER_BLOB) -> Result<Vec<u8>, SecretError> {
        if blob.cbData != 0 && blob.pbData.is_null() {
            return Err(SecretError::Dpapi(0));
        }
        let bytes = if blob.cbData == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec() }
        };
        if !blob.pbData.is_null() {
            unsafe { LocalFree(blob.pbData.cast()) };
        }
        Ok(bytes)
    }
}
