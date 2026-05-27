//! Provides an interface for querying data from the nix store.
//!
//! - [`DbConnection`] is a direct connection to the underlying sqlite database.
//! - [`CommandBackend`] uses nix commands to interact with the store.
mod db;
pub mod nix_command;
mod queries;
// Make the test db available for the rest of the crate.
#[cfg(test)] pub(crate) mod test_utils;

use std::{
  fmt::Display,
  path::Path,
};

pub use db::DbConnection;
use eyre::{
  Result,
  eyre,
};
pub use nix_command::CommandBackend;
use size::Size;
use tracing::warn;

use crate::StorePath;
/// The normal database connection
pub const DATABASE_PATH: &str = "file:/nix/var/nix/db/db.sqlite";
/// A backup database connection that can access the database
/// even in a read-only environment
///
/// This might produce incorrect results as the connection is not guaranteed
/// to be the only one accessing the database. (There might be e.g. a
/// `nixos-rebuild` modifying the database)
pub const DATABASE_PATH_IMMUTABLE: &str =
  "file:/nix/var/nix/db/db.sqlite?immutable=1";

/// Defines an interface for interacting with a Nix database.
///
/// This allows us to construct a backend that can fall back
/// to e.g. shell commands should something go wrong.
pub trait StoreBackend: Display {
  /// Opens any resources required by the backend.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend cannot be connected.
  fn connect(&mut self) -> Result<()>;
  /// Returns whether this backend is currently connected.
  fn connected(&self) -> bool;
  /// Closes resources held by the backend.
  ///
  /// # Errors
  ///
  /// Returns an error if resources cannot be closed cleanly.
  fn close(&mut self) -> Result<()>;
  /// Queries the closure size for a Nix store path.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails or the size cannot be read.
  fn query_closure_size(&self, path: &Path) -> Result<Size>;
  /// Queries derivations selected by a system profile.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails.
  fn query_system_derivations(&self, system: &Path) -> Result<Vec<StorePath>>;
  /// Queries all dependencies of a store path.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails.
  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>>;
}

/// combines multiple store backends by falling back to the next one if the
/// current one fails.
///
/// Queries try connected backends in order.
pub struct CombinedStoreBackend {
  /// The underlying store backend implementations.
  backends: Vec<Box<dyn StoreBackend>>,
}

impl CombinedStoreBackend {
  #[must_use]
  pub fn new(backends: Vec<Box<dyn StoreBackend>>) -> Self {
    Self { backends }
  }

  #[must_use]
  pub fn for_correctness(force_correctness: bool) -> Self {
    if force_correctness {
      Self::default_correct()
    } else {
      Self::default_fast()
    }
  }

  pub(crate) fn query_with_correctness<T>(
    force_correctness: bool,
    query: impl FnOnce(&Self) -> Result<T>,
  ) -> Result<T> {
    let mut backend = Self::for_correctness(force_correctness);
    backend.connect()?;

    let result = query(&backend);
    let close_result = backend.close();

    match result {
      Ok(value) => {
        close_result?;
        Ok(value)
      },
      Err(error) => Err(error),
    }
  }

  /// Returns a backend that is focused on performance and availability.
  ///
  /// This first tries the regular sqlite database, then falls back to opening
  /// it with `immutable=1`, and finally falls back to Nix commands.
  #[must_use]
  pub fn default_fast() -> Self {
    Self::new(vec![
      Box::new(DbConnection::new(DATABASE_PATH)),
      Box::new(DbConnection::new(DATABASE_PATH_IMMUTABLE)),
      Box::new(CommandBackend::default()),
    ])
  }

  /// Returns a backend that is focused solely on absolutely guaranteeing
  /// correct results if the regular sqlite database cannot be opened.
  ///
  /// Note that [`DATABASE_PATH_IMMUTABLE`] is not used here, since opening
  /// the database can lead to undefined results (also silently with no errors)
  /// if the database is actually modified while opened.
  #[must_use]
  pub fn default_correct() -> Self {
    Self::new(vec![
      Box::new(DbConnection::new(DATABASE_PATH)),
      Box::new(CommandBackend::default()),
    ])
  }

  // tries to execute a query until it succeeds or all connected backends have
  // been tried
  fn fallback_query<F, Ret>(&self, query: F, path: &Path) -> Result<Ret>
  where
    F: Fn(&dyn StoreBackend, &Path) -> Result<Ret>,
  {
    let mut combined_err: Option<eyre::Report> = None;
    // attempt to cycle through backends until a successful query is made
    for (i, backend) in self.backends.iter().enumerate() {
      if !backend.connected() {
        warn!(
          "Skipping backend {i} ({backend}) in query {path:?}: not connected"
        );
        continue;
      }
      let res = query(backend.as_ref(), path);
      match res {
        Ok(_) => return res,
        Err(err) => {
          warn!(
            "Failed to query path {path:?} on current backend {backend} \
             ({i}): {}",
            &err
          );
          combined_err = match combined_err {
            Some(combined) => Some(combined.wrap_err(err)),
            None => Some(err),
          };
        },
      }
    }
    warn!("All store backends for path {path:?} failed");
    Err(combined_err.unwrap_or_else(|| eyre!("No internal stores to query.")))
  }
}

impl Display for CombinedStoreBackend {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "CombinedStoreBackend({} backends)", self.backends.len())
  }
}

impl Default for CombinedStoreBackend {
  fn default() -> Self {
    Self::default_fast()
  }
}

impl StoreBackend for CombinedStoreBackend {
  /// connects to all backends. Returns an error if all backends fail
  fn connect(&mut self) -> Result<()> {
    tracing::debug!(
      backend_count = self.backends.len(),
      "connecting to store backends"
    );
    let mut combined_err: Option<eyre::Report> = None;
    let mut connected_count = 0;
    // connect, collecting the errors as we go
    for (i, backend) in self.backends.iter_mut().enumerate() {
      tracing::trace!(backend_index = i, backend = %backend, "attempting to connect to backend");
      if let Err(err) = backend.connect() {
        warn!(
          "Unable to connect to store backend {i}: {backend}, trying next. \
           (error: {err})"
        );
        combined_err = match combined_err {
          Some(combined) => Some(combined.wrap_err(err)),
          None => Some(err),
        }
      } else {
        connected_count += 1;
        tracing::debug!(backend_index = i, backend = %backend, "backend connected successfully");
      }
    }
    tracing::info!(
      connected_count = connected_count,
      total_count = self.backends.len(),
      "backend connection complete"
    );
    let any_succeeded = self.backends.iter().any(|f| f.connected());
    // warn about encountered errors, even though there are fallbacks
    if let Some(err) = &combined_err
      && any_succeeded
    {
      warn!("Some backends failed to connect: {err}");
    }
    if any_succeeded {
      Ok(())
    } else {
      combined_err =
        combined_err.map(|err| err.wrap_err("All backends failed to connect."));
      Err(combined_err.unwrap_or_else(|| eyre!("No backends to connect to.")))
    }
  }

  /// True if any backend is connected.
  fn connected(&self) -> bool {
    self.backends.iter().any(|backend| backend.connected())
  }

  /// Closes all connected backends.
  ///
  /// If some fail to close, the combined error is returned.
  fn close(&mut self) -> Result<()> {
    let mut combined_err: Option<eyre::Report> = None;
    for (i, backend) in self.backends.iter_mut().enumerate() {
      if backend.connected()
        && let Err(err) = backend.close()
      {
        warn!("Unable to close store backend {i}: {backend}. (error: {err})");
        combined_err = match combined_err {
          Some(combined) => Some(combined.wrap_err(err)),
          None => Some(err),
        };
      }
    }
    combined_err.map_or_else(
      || Ok(()),
      |err| Err(err.wrap_err("One or more backends failed to close.")),
    )
  }

  fn query_closure_size(&self, path: &Path) -> Result<Size> {
    self.fallback_query(|backend, path| backend.query_closure_size(path), path)
  }

  fn query_system_derivations(&self, system: &Path) -> Result<Vec<StorePath>> {
    self.fallback_query(
      |backend, system| backend.query_system_derivations(system),
      system,
    )
  }

  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>> {
    self.fallback_query(|backend, path| backend.query_dependents(path), path)
  }
}

#[cfg(test)]
mod test {
  use std::{
    cell::RefCell,
    fmt,
  };

  use super::*;

  struct MockStoreBackend {
    name:         String,
    connected:    bool,
    fail_connect: bool,
    fail_query:   bool,
    query_called: RefCell<bool>,
  }

  impl MockStoreBackend {
    fn new(name: &str, fail_connect: bool, fail_query: bool) -> Self {
      Self {
        name: name.to_string(),
        connected: false,
        fail_connect,
        fail_query,
        query_called: RefCell::new(false),
      }
    }
  }

  impl Display for MockStoreBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      write!(f, "MockStoreBackend({})", self.name)
    }
  }

  impl StoreBackend for MockStoreBackend {
    fn connect(&mut self) -> Result<()> {
      if self.fail_connect {
        Err(eyre!("Connection failed"))
      } else {
        self.connected = true;
        Ok(())
      }
    }

    fn connected(&self) -> bool {
      self.connected
    }

    fn close(&mut self) -> Result<()> {
      self.connected = false;
      Ok(())
    }

    fn query_closure_size(&self, _path: &Path) -> Result<Size> {
      *self.query_called.borrow_mut() = true;
      if self.fail_query {
        Err(eyre!("Query failed"))
      } else {
        Ok(Size::from_bytes(100))
      }
    }

    fn query_system_derivations(
      &self,
      _system: &Path,
    ) -> Result<Vec<StorePath>> {
      *self.query_called.borrow_mut() = true;
      if self.fail_query {
        Err(eyre!("Query failed"))
      } else {
        Ok(Vec::new())
      }
    }

    fn query_dependents(&self, _path: &Path) -> Result<Vec<StorePath>> {
      *self.query_called.borrow_mut() = true;
      if self.fail_query {
        Err(eyre!("Query failed"))
      } else {
        Ok(Vec::new())
      }
    }
  }

  #[test]
  fn test_connect_fallback() {
    let f1 = Box::new(MockStoreBackend::new("f1", true, false));
    let f2 = Box::new(MockStoreBackend::new("f2", false, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    assert!(combined.connect().is_ok());
    assert!(combined.connected());
  }

  #[test]
  fn test_connect_all_fail() {
    let f1 = Box::new(MockStoreBackend::new("f1", true, false));
    let f2 = Box::new(MockStoreBackend::new("f2", true, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    assert!(combined.connect().is_err());
    assert!(!combined.connected());
  }

  #[test]
  fn test_query_fallback() {
    let f1 = Box::new(MockStoreBackend::new("f1", false, true));
    let f2 = Box::new(MockStoreBackend::new("f2", false, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    combined.connect().unwrap();

    let res = combined.query_closure_size(Path::new("/dummy"));
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), Size::from_bytes(100));
  }

  #[test]
  fn test_path_query_fallback() {
    let f1 = Box::new(MockStoreBackend::new("f1", false, true));
    let f2 = Box::new(MockStoreBackend::new("f2", false, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    combined.connect().unwrap();

    let res = combined.query_dependents(Path::new("/dummy"));
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), Vec::new());
  }

  #[test]
  fn test_query_skip_unconnected() {
    let f1 = Box::new(MockStoreBackend::new("f1", true, false));
    let f2 = Box::new(MockStoreBackend::new("f2", false, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    combined.connect().unwrap(); // f1 fails, f2 succeeds

    let res = combined.query_closure_size(Path::new("/dummy"));
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), Size::from_bytes(100));

    let f1 = Box::new(MockStoreBackend::new("f1", true, false));
    let f2 = Box::new(MockStoreBackend::new("f2", true, false));
    let f3 = Box::new(MockStoreBackend::new("f3", false, false));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2, f3]);
    combined.connect().unwrap();

    let res = combined.query_closure_size(Path::new("/dummy"));
    assert_eq!(res.unwrap(), Size::from_bytes(100));
    assert!(combined.connect().is_ok());
    assert!(combined.connected());
  }

  #[test]
  fn test_query_all_fail() {
    let f1 = Box::new(MockStoreBackend::new("f1", false, true));
    let f2 = Box::new(MockStoreBackend::new("f2", false, true));
    let mut combined = CombinedStoreBackend::new(vec![f1, f2]);

    combined.connect().unwrap();

    let res = combined.query_closure_size(Path::new("/dummy"));
    assert!(res.is_err());
  }
}
