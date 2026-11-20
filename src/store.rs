//! Provides an interface for querying data from the nix store.
//!
//! - [`DbConnection`] is a direct connection to the underlying sqlite database.
//! - [`CommandBackend`] uses nix commands to interact with the store.
mod db;
pub mod nix_command;
mod queries;
// Make the test db available for the rest of the crate.
#[cfg(test)] pub mod test_utils;

use std::{
  error,
  path::Path,
};

pub use db::DbConnection;
pub use nix_command::CommandBackend;
use tracing::warn;

use crate::{
  Error,
  SnapshotDocument,
  StorePath,
  StorePathInfo,
  StoreSnapshot,
};

/// The normal database connection
pub const DATABASE_PATH: &str = "file:/nix/var/nix/db/db.sqlite";

/// Queries Nix store data for one path using a caller-provided backend.
///
/// # Returns
///
/// The closure of `path` with NAR sizes, and the packages it selects.
///
/// # Errors
///
/// Returns an error if the backend cannot query the path, or the path is not
/// a valid store path.
fn query_store_snapshot_with_backend(
  backend: &impl StoreBackend<Error = Error>,
  path: &StorePath,
) -> Result<StoreSnapshot, Error> {
  tracing::debug!(path = %path.display(), "querying closure path info");
  let closure = backend.query_closure_path_info(path).map_err(|source| {
    Error::ClosureQuery {
      path:   path.clone(),
      source: Box::new(source),
    }
  })?;
  // The closure of a valid path contains at least the path itself.
  if closure.is_empty() {
    return Err(Error::PathNotValid(path.clone()));
  }

  tracing::debug!(path = %path.display(), "querying system derivations");
  let selected = backend.query_system_derivations(path).map_err(|source| {
    Error::SelectionQuery {
      path:   path.clone(),
      source: Box::new(source),
    }
  })?;

  Ok(StoreSnapshot { closure, selected })
}

/// A handle to the local Nix store.
///
/// It queries the Nix database, and falls back to Nix commands if the database
/// cannot be opened or queried. Reuse one handle for several queries, so the
/// database is opened only once.
pub struct Store {
  /// The Nix database, if it could be opened.
  db: Option<DbConnection>,
}

impl Store {
  /// Opens the local Nix store.
  ///
  /// The database is never opened with `?immutable=1`, since that can silently
  /// produce wrong results while Nix writes to it. If it cannot be opened, a
  /// warning is logged and queries use Nix commands.
  #[must_use]
  pub fn open() -> Self {
    let db = DbConnection::open(DATABASE_PATH)
      .inspect_err(|err| {
        warn!(
          "unable to open the Nix database at {DATABASE_PATH}, falling back: \
           {}",
          error_chain(err)
        );
      })
      .ok();
    Self { db }
  }

  /// Canonicalizes `path` and queries a
  /// [`SnapshotDocument`](crate::SnapshotDocument) for it.
  ///
  /// # Returns
  ///
  /// A snapshot of the store path that `path` resolves to.
  ///
  /// # Errors
  ///
  /// Returns an error if the path cannot be canonicalized, does not resolve to
  /// a store path, or the store queries fail.
  pub fn query_snapshot_document(
    &self,
    path: &Path,
  ) -> Result<SnapshotDocument, Error> {
    let canonical = path.canonicalize().map_err(|source| {
      Error::Canonicalize {
        path: path.to_owned(),
        source,
      }
    })?;
    let store_path = canonical
      .into_os_string()
      .into_string()
      .map_err(|path| Error::NotUnicode(path.into()))
      .and_then(StorePath::try_from)?;
    let snapshot = query_store_snapshot_with_backend(self, &store_path)?;
    Ok(SnapshotDocument {
      path: store_path,
      snapshot,
    })
  }

  /// Runs `query` on the database, if it is open.
  ///
  /// # Returns
  ///
  /// The query's result, or `None` if the database is not open or the query
  /// failed, in which case a warning is logged.
  fn query_db<T>(
    &self,
    path: &StorePath,
    query: impl FnOnce(&DbConnection) -> rusqlite::Result<T>,
  ) -> Option<T> {
    let db = self.db.as_ref()?;
    query(db)
      .inspect_err(|err| {
        warn!(
          "failed to query '{}' with {db}, falling back: {}",
          path.display(),
          error_chain(err)
        );
      })
      .ok()
  }
}

impl StoreBackend for Store {
  type Error = Error;

  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> Result<Vec<StorePath>, Error> {
    self
      .query_db(system, |db| db.query_system_derivations(system))
      .map_or_else(|| CommandBackend.query_system_derivations(system), Ok)
  }

  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> Result<Vec<StorePathInfo>, Error> {
    self
      .query_db(path, |db| db.query_closure_path_info(path))
      .map_or_else(|| CommandBackend.query_closure_path_info(path), Ok)
  }
}

/// Defines an interface for querying the Nix store.
pub trait StoreBackend {
  /// The error queries return.
  type Error: error::Error + Send + Sync + 'static;

  /// Queries derivations selected by a system profile.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails.
  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> Result<Vec<StorePath>, Self::Error>;
  /// Queries all dependencies of a store path with their NAR sizes.
  ///
  /// # Returns
  ///
  /// The closure of `path`, or an empty list if `path` is not valid.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails or path information cannot be
  /// read.
  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> Result<Vec<StorePathInfo>, Self::Error>;
}

/// Renders `error` and its sources, joined with `: `, for log messages.
fn error_chain(error: &dyn error::Error) -> String {
  let mut rendered = error.to_string();
  let mut source = error.source();
  while let Some(error) = source {
    rendered.push_str(": ");
    rendered.push_str(&error.to_string());
    source = error.source();
  }
  rendered
}
