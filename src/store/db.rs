use std::fmt::{
  self,
  Display,
};

use rusqlite::{
  Connection,
  OpenFlags,
  types::Type,
};
use size::Size;

use crate::{
  StorePath,
  StorePathInfo,
  store::{
    StoreBackend,
    queries,
  },
};

#[derive(Debug)]
pub struct DbConnection {
  path: String,
  conn: Connection,
}

impl Display for DbConnection {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "DBConnection({})", self.path)
  }
}

impl DbConnection {
  /// Opens a read-only connection to the database at `path`.
  ///
  /// # Errors
  ///
  /// Returns an error if the database cannot be opened or configured.
  pub fn open(path: impl AsRef<str>) -> rusqlite::Result<Self> {
    let path = path.as_ref().to_owned();
    let conn = open_connection(&path)?;
    Ok(Self { path, conn })
  }
}

impl StoreBackend for DbConnection {
  type Error = rusqlite::Error;

  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> rusqlite::Result<Vec<StorePath>> {
    query_store_paths(&self.conn, queries::QUERY_SYSTEM_DERIVATIONS, system)
  }

  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> rusqlite::Result<Vec<StorePathInfo>> {
    query_store_path_info(&self.conn, path)
  }
}

fn open_connection(path: &str) -> rusqlite::Result<Connection> {
  tracing::debug!(database_path = path, "opening sqlite connection");
  let inner = Connection::open_with_flags(
    path,
    OpenFlags::SQLITE_OPEN_READ_ONLY // We only run queries, safeguard against corrupting the DB.
      | OpenFlags::SQLITE_OPEN_NO_MUTEX // Part of the default flags, rusqlite takes care of locking anyways.
      | OpenFlags::SQLITE_OPEN_URI,
  )?;
  tracing::debug!(
    database_path = path,
    "sqlite connection opened successfully"
  );

  // Perform a batched query to set some settings using PRAGMA
  // the main performance bottleneck when dix was run before
  // was that the database file has to be brought from disk into
  // memory.
  //
  // We read a large part of the DB anyways in each query,
  // so it makes sense to set aside a large region of memory-mapped
  // I/O prevent incurring page faults which can be done using
  // `mmap_size`.
  //
  // This made a performance difference of about 500ms (but only
  // when it was first run for a long time!).
  //
  // The file pages of the store can be evicted from main memory
  // using:
  //
  // ```bash
  // dd of=/nix/var/nix/db/db.sqlite oflag=nocache conv=notrunc,fdatasync count=0
  // ```
  //
  // If you want to test this. Source: <https://unix.stackexchange.com/questions/36907/drop-a-specific-file-from-the-linux-filesystem-cache>.
  //
  // Documentation about the settings can be found here: <https://www.sqlite.org/pragma.html>
  //
  // [0]: 256MB, enough to fit the whole DB (at least on my system - Dragyx).
  // [1]: Always store temporary tables in memory.
  inner.execute_batch(
    "
        PRAGMA mmap_size=268435456; -- See [0].
        PRAGMA temp_store=2; -- See [1].
        PRAGMA query_only;
      ",
  )?;
  Ok(inner)
}

fn query_store_paths(
  conn: &Connection,
  query: &str,
  path: &StorePath,
) -> rusqlite::Result<Vec<StorePath>> {
  let mut query = conn.prepare_cached(query)?;
  query
    .query_map([path.as_str()], |row| store_path(row.get(0)?))?
    .collect()
}

/// Validates a store path read from the database.
///
/// # Errors
///
/// Returns a conversion error if `path` is not a store path.
fn store_path(path: String) -> rusqlite::Result<StorePath> {
  StorePath::try_from(path).map_err(|err| {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(err))
  })
}

fn query_store_path_info(
  conn: &Connection,
  path: &StorePath,
) -> rusqlite::Result<Vec<StorePathInfo>> {
  let mut query = conn.prepare_cached(queries::QUERY_CLOSURE_PATH_INFO)?;
  query
    .query_map([path.as_str()], |row| {
      Ok(StorePathInfo {
        path:     store_path(row.get(0)?)?,
        nar_size: Size::from_bytes(row.get::<_, i64>(1)?),
      })
    })?
    .collect()
}
