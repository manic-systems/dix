//! Provides an interface for querying data from the nix store.
//!
//! - [`DbConnection`] is a direct connection to the underlying sqlite database.
//! - [`CommandBackend`] uses nix commands to interact with the store.
mod db;
pub mod nix_command;
mod queries;
// Make the test db available for the rest of the crate.
#[cfg(test)] pub mod test_utils;

use std::fmt::Display;

pub use db::DbConnection;
use misstep::{
  Result,
  report,
};
pub use nix_command::CommandBackend;
use serde::{
  Deserialize,
  Serialize,
};
use size::Size;
use tracing::warn;

use crate::StorePath;

/// The normal database connection
pub const DATABASE_PATH: &str = "file:/nix/var/nix/db/db.sqlite";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorePathInfo {
  pub path:     StorePath,
  #[serde(with = "size_bytes")]
  pub nar_size: Size,
}

/// (De)serializes a [`Size`] as a plain byte count.
mod size_bytes {
  use serde::{
    Deserialize as _,
    Deserializer,
    Serializer,
  };
  use size::Size;

  /// Serializes `size` as its byte count.
  ///
  /// # Returns
  ///
  /// The serializer's output.
  ///
  /// # Errors
  ///
  /// Returns the serializer's error if writing the number fails.
  #[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature required by `serde(with)`"
  )]
  pub fn serialize<S: Serializer>(
    size: &Size,
    serializer: S,
  ) -> Result<S::Ok, S::Error> {
    serializer.serialize_i64(size.bytes())
  }

  /// Deserializes a [`Size`] from its byte count.
  ///
  /// # Returns
  ///
  /// The size.
  ///
  /// # Errors
  ///
  /// Returns the deserializer's error if the value is not an integer.
  pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
  ) -> Result<Size, D::Error> {
    i64::deserialize(deserializer).map(Size::from_bytes)
  }
}

/// Defines an interface for interacting with a Nix database.
///
/// This allows us to construct a backend that can fall back
/// to e.g. shell commands should something go wrong.
pub trait StoreBackend: Display {
  /// Queries derivations selected by a system profile.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails.
  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> Result<Vec<StorePath>>;
  /// Queries all dependencies of a store path with their NAR sizes.
  ///
  /// # Errors
  ///
  /// Returns an error if the backend query fails or path information cannot be
  /// read.
  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> Result<Vec<StorePathInfo>>;
}

thread_local! {
  /// The backend shared by all store queries on this thread, so the Nix
  /// database is opened, and a failure to open it reported, only once.
  pub static STORE: CombinedStoreBackend = CombinedStoreBackend::open();
}

/// combines multiple store backends by falling back to the next one if the
/// current one fails.
///
/// Queries try backends in order.
pub struct CombinedStoreBackend {
  /// The underlying store backend implementations.
  backends: Vec<Box<dyn StoreBackend>>,
}

impl CombinedStoreBackend {
  #[must_use]
  pub fn new(backends: Vec<Box<dyn StoreBackend>>) -> Self {
    Self { backends }
  }

  /// Tries the sqlite database first, then falls back to Nix commands.
  ///
  /// The database is never opened with `?immutable=1`, since that can silently
  /// produce wrong results while Nix writes to it.
  fn open() -> Self {
    let mut backends: Vec<Box<dyn StoreBackend>> = Vec::new();
    match DbConnection::open(DATABASE_PATH) {
      Ok(db) => backends.push(Box::new(db)),
      Err(err) => warn!("Unable to open the Nix database, falling back: {err}"),
    }
    backends.push(Box::new(CommandBackend));
    Self::new(backends)
  }

  // tries to execute a query until it succeeds or all backends have been tried
  fn fallback_query<F, Ret>(&self, query: F, path: &StorePath) -> Result<Ret>
  where
    F: Fn(&dyn StoreBackend, &StorePath) -> Result<Ret>,
  {
    let mut combined_err: Option<misstep::Report> = None;
    // attempt to cycle through backends until a successful query is made
    for (i, backend) in self.backends.iter().enumerate() {
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
            Some(combined) => Some(combined.context(err)),
            None => Some(err),
          };
        },
      }
    }
    warn!("All store backends for path {path:?} failed");
    Err(combined_err.unwrap_or_else(|| report!("No internal stores to query.")))
  }
}

impl Display for CombinedStoreBackend {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "CombinedStoreBackend({} backends)", self.backends.len())
  }
}

impl StoreBackend for CombinedStoreBackend {
  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> Result<Vec<StorePath>> {
    self.fallback_query(
      |backend, system| backend.query_system_derivations(system),
      system,
    )
  }

  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> Result<Vec<StorePathInfo>> {
    self.fallback_query(
      |backend, path| backend.query_closure_path_info(path),
      path,
    )
  }
}

#[cfg(test)]
mod test {
  use std::fmt;

  use super::*;

  fn dummy_path() -> StorePath {
    StorePath::try_from(String::from(
      "/nix/store/0123456789abcdefghijklmnopqrstuv-dummy",
    ))
    .unwrap()
  }

  struct MockStoreBackend {
    name:       String,
    fail_query: bool,
  }

  impl MockStoreBackend {
    fn new(name: &str, fail_query: bool) -> Self {
      Self {
        name: name.to_string(),
        fail_query,
      }
    }
  }

  impl Display for MockStoreBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      write!(f, "MockStoreBackend({})", self.name)
    }
  }

  impl StoreBackend for MockStoreBackend {
    fn query_system_derivations(
      &self,
      _system: &StorePath,
    ) -> Result<Vec<StorePath>> {
      if self.fail_query {
        Err(report!("Query failed"))
      } else {
        Ok(Vec::new())
      }
    }

    fn query_closure_path_info(
      &self,
      _path: &StorePath,
    ) -> Result<Vec<StorePathInfo>> {
      if self.fail_query {
        Err(report!("Query failed"))
      } else {
        Ok(vec![StorePathInfo {
          path:     dummy_path(),
          nar_size: Size::from_bytes(100),
        }])
      }
    }
  }

  #[test]
  fn test_query_fallback() {
    let f1 = Box::new(MockStoreBackend::new("f1", true));
    let f2 = Box::new(MockStoreBackend::new("f2", false));
    let combined = CombinedStoreBackend::new(vec![f1, f2]);

    let res = test_utils::closure_size(&combined, &dummy_path());
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), Size::from_bytes(100));
  }

  #[test]
  fn test_path_query_fallback() {
    let f1 = Box::new(MockStoreBackend::new("f1", true));
    let f2 = Box::new(MockStoreBackend::new("f2", false));
    let combined = CombinedStoreBackend::new(vec![f1, f2]);

    let res = combined.query_system_derivations(&dummy_path());
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), Vec::new());
  }

  #[test]
  fn test_query_all_fail() {
    let f1 = Box::new(MockStoreBackend::new("f1", true));
    let f2 = Box::new(MockStoreBackend::new("f2", true));
    let combined = CombinedStoreBackend::new(vec![f1, f2]);

    let res = test_utils::closure_size(&combined, &dummy_path());
    assert!(res.is_err());
  }
}
