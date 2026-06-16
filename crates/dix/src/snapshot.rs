use std::path::Path;

use eyre::{
  Context as _,
  Result,
};

use crate::{
  StorePath,
  store::{
    CombinedStoreBackend,
    StoreBackend,
    StorePathInfo,
  },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSnapshot {
  pub closure:  Vec<StorePathInfo>,
  pub selected: Vec<StorePath>,
}

/// Queries Nix store data for one path and returns a reusable snapshot.
///
/// # Errors
///
/// Returns an error if the store connection or path queries fail.
pub fn query_store_snapshot(
  path: &Path,
  force_correctness: bool,
) -> Result<StoreSnapshot> {
  CombinedStoreBackend::query_with_correctness(force_correctness, |backend| {
    query_store_snapshot_with_backend(backend, path)
  })
}

/// Queries Nix store data for one path using a caller-provided backend.
///
/// This does not call [`StoreBackend::connect`] or [`StoreBackend::close`].
/// Callers using connection-backed implementations must manage that lifecycle.
///
/// # Errors
///
/// Returns an error if the backend cannot query the path.
pub fn query_store_snapshot_with_backend(
  backend: &dyn StoreBackend,
  path: &Path,
) -> Result<StoreSnapshot> {
  tracing::debug!(path = %path.display(), "querying closure path info");
  let closure = backend.query_closure_path_info(path).with_context(|| {
    format!("failed to query closure path info of '{}'", path.display())
  })?;

  tracing::debug!(path = %path.display(), "querying system derivations");
  let selected = backend.query_system_derivations(path).with_context(|| {
    format!("failed to query system derivations of '{}'", path.display())
  })?;

  Ok(StoreSnapshot { closure, selected })
}
