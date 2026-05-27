use std::{
  fmt::{
    self,
    Display,
  },
  path::{
    Path,
    PathBuf,
  },
};

use eyre::{
  Context as _,
  Result,
  eyre,
};
use rusqlite::{
  Connection,
  OpenFlags,
};
use size::Size;

use crate::{
  StorePath,
  path_to_canonical_string,
  store::{
    StoreBackend,
    StorePathSnapshot,
    queries,
  },
};

#[derive(Debug)]
pub struct DbConnection {
  path: String,
  conn: Option<Connection>,
}

impl Display for DbConnection {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "DBConnection({})", self.path)
  }
}

impl DbConnection {
  /// Create a new connection.
  pub fn new(path: impl AsRef<str>) -> Self {
    Self {
      path: path.as_ref().to_owned(),
      conn: None,
    }
  }

  /// returns a reference to the inner connection
  ///
  /// raises an error if the connection has not been established
  fn get_inner(&self) -> Result<&Connection> {
    self
      .conn
      .as_ref()
      .ok_or_else(|| eyre!("Attempted to use database before connecting."))
  }
}

impl StoreBackend for DbConnection {
  fn connect(&mut self) -> Result<()> {
    self.conn = Some(open_connection(&self.path)?);
    Ok(())
  }

  fn connected(&self) -> bool {
    self.conn.is_some()
  }

  fn close(&mut self) -> Result<()> {
    close_inner_connection(&self.path, &mut self.conn)
  }

  fn query_closure_size(&self, path: &Path) -> Result<size::Size> {
    query_closure_size(self.get_inner()?, path)
  }

  fn query_system_derivations(&self, system: &Path) -> Result<Vec<StorePath>> {
    query_store_paths(
      self.get_inner()?,
      queries::QUERY_SYSTEM_DERIVATIONS,
      system,
    )
  }

  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>> {
    query_store_paths(self.get_inner()?, queries::QUERY_DEPENDENTS, path)
  }

  fn query_path_snapshot(&self, path: &Path) -> Result<StorePathSnapshot> {
    query_path_snapshot(self.get_inner()?, path)
  }
}

fn open_connection(path: &str) -> Result<Connection> {
  tracing::debug!(database_path = path, "opening sqlite connection");
  let inner = Connection::open_with_flags(
    path,
    OpenFlags::SQLITE_OPEN_READ_ONLY // We only run queries, safeguard against corrupting the DB.
      | OpenFlags::SQLITE_OPEN_NO_MUTEX // Part of the default flags, rusqlite takes care of locking anyways.
      | OpenFlags::SQLITE_OPEN_URI,
  )
  .with_context(|| format!("failed to connect to Nix database at {path}"))?;
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
  inner
    .execute_batch(
      "
        PRAGMA mmap_size=268435456; -- See [0].
        PRAGMA temp_store=2; -- See [1].
        PRAGMA query_only;
      ",
    )
    .with_context(|| format!("failed to cache Nix database at {path}"))?;
  Ok(inner)
}

fn close_inner_connection(
  path: &str,
  maybe_conn: &mut Option<Connection>,
) -> Result<()> {
  let conn = maybe_conn.take().ok_or_else(|| {
    eyre!("Tried to close connection to {} that does not exist", path)
  })?;
  conn.close().map_err(|(conn_old, err)| {
    *maybe_conn = Some(conn_old);
    eyre::Report::from(err).wrap_err("failed to close Nix database")
  })
}

fn query_closure_size(conn: &Connection, path: &Path) -> Result<Size> {
  tracing::trace!(path = %path.display(), "querying closure size");
  let path = path_to_canonical_string(path)?;

  let closure_size = conn
    .prepare_cached(queries::QUERY_CLOSURE_SIZE)?
    .query_row([path], |row| Ok(Size::from_bytes(row.get::<_, i64>(0)?)))?;

  Ok(closure_size)
}

fn query_store_paths(
  conn: &Connection,
  query: &str,
  path: &Path,
) -> Result<Vec<StorePath>> {
  let path = path_to_canonical_string(path)?;
  let mut query = conn.prepare_cached(query)?;
  let rows = query.query_map([path], |row| row.get::<_, String>(0))?;

  let mut paths = Vec::new();
  for row in rows {
    paths.push(StorePath::try_from(PathBuf::from(row?))?);
  }

  Ok(paths)
}

fn query_path_snapshot(
  conn: &Connection,
  path: &Path,
) -> Result<StorePathSnapshot> {
  const DEPENDENCY: i64 = 0;
  const SELECTED: i64 = 1;
  const CLOSURE_SIZE: i64 = 2;

  let path = path_to_canonical_string(path)?;
  let mut query = conn.prepare_cached(queries::QUERY_PATH_SNAPSHOT)?;
  let mut rows = query.query([path])?;
  let mut dependencies = Vec::new();
  let mut selected = Vec::new();
  let mut closure_size = None;

  while let Some(row) = rows.next()? {
    match row.get::<_, i64>(0)? {
      DEPENDENCY => {
        let path = row.get::<_, String>(1)?;
        dependencies.push(StorePath::try_from(PathBuf::from(path))?);
      },
      SELECTED => {
        let path = row.get::<_, String>(1)?;
        selected.push(StorePath::try_from(PathBuf::from(path))?);
      },
      CLOSURE_SIZE => {
        closure_size = Some(Size::from_bytes(row.get::<_, i64>(2)?));
      },
      kind => return Err(eyre!("unexpected path snapshot row kind {kind}")),
    }
  }

  Ok(StorePathSnapshot {
    dependencies,
    selected,
    closure_size: closure_size.ok_or_else(|| {
      eyre!("path snapshot query did not return closure size")
    })?,
  })
}
