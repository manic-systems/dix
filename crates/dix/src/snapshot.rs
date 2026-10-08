use std::path::Path;

use eyre::{
  Context as _,
  Result,
  bail,
};
use serde::{
  Deserialize,
  Serialize,
};

use crate::{
  StorePath,
  store::{
    CombinedStoreBackend,
    StoreBackend,
    StorePathInfo,
  },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// Returns an error if the backend cannot query the path, or the path is not
/// a valid store path.
pub fn query_store_snapshot_with_backend(
  backend: &dyn StoreBackend,
  path: &Path,
) -> Result<StoreSnapshot> {
  tracing::debug!(path = %path.display(), "querying closure path info");
  let closure = backend.query_closure_path_info(path).with_context(|| {
    format!("failed to query closure path info of '{}'", path.display())
  })?;
  // The closure of a valid path contains at least the path itself.
  if closure.is_empty() {
    bail!("'{}' is not a valid store path", path.display());
  }

  tracing::debug!(path = %path.display(), "querying system derivations");
  let selected = backend.query_system_derivations(path).with_context(|| {
    format!("failed to query system derivations of '{}'", path.display())
  })?;

  Ok(StoreSnapshot { closure, selected })
}

/// Version of the [`SnapshotDocument`] JSON format.
///
/// Bump this whenever the format changes in a way older readers cannot handle.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// A versioned, serializable snapshot of a single store path's closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDocument {
  pub version:  u32,
  /// The canonical store path the snapshot was taken of.
  pub path:     StorePath,
  #[serde(flatten)]
  pub snapshot: StoreSnapshot,
}

impl SnapshotDocument {
  #[must_use]
  pub const fn new(path: StorePath, snapshot: StoreSnapshot) -> Self {
    Self {
      version: SNAPSHOT_FORMAT_VERSION,
      path,
      snapshot,
    }
  }

  /// Parses a document from JSON.
  ///
  /// The format version is checked before anything else, so documents written
  /// by an incompatible dix fail with a version error instead of a field error.
  ///
  /// # Errors
  ///
  /// Returns an error if the input is not a snapshot document, has an
  /// unsupported format version, or contains invalid data.
  pub fn from_json(input: &str) -> Result<Self> {
    #[derive(Deserialize)]
    struct VersionProbe {
      version: u32,
    }

    let VersionProbe { version } = serde_json::from_str(input)
      .context("input is not a dix snapshot document")?;
    if version != SNAPSHOT_FORMAT_VERSION {
      bail!(
        "unsupported dix snapshot format version {version} (supported: \
         {SNAPSHOT_FORMAT_VERSION})"
      );
    }

    serde_json::from_str(input).context("failed to parse dix snapshot document")
  }
}

/// Canonicalizes `path` and queries a [`SnapshotDocument`] for it.
///
/// Always uses the correctness-focused backend chain, since snapshots are
/// meant to be consumed by other programs.
///
/// # Errors
///
/// Returns an error if the path cannot be canonicalized, does not resolve to
/// a store path, or the store queries fail.
pub fn query_snapshot_document(path: &Path) -> Result<SnapshotDocument> {
  let canonical = path.canonicalize().with_context(|| {
    format!("failed to canonicalize path '{}'", path.display())
  })?;
  let store_path = StorePath::try_from(canonical)?;
  let snapshot = query_store_snapshot(&store_path, true)?;
  Ok(SnapshotDocument::new(store_path, snapshot))
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use size::Size;

  use super::*;

  fn store_path(hash: u8, name: &str) -> StorePath {
    StorePath::try_from(PathBuf::from(format!("/nix/store/{hash:0>32}-{name}")))
      .unwrap()
  }

  fn document() -> SnapshotDocument {
    let system = store_path(1, "nixos-system-25.11");
    let bash = store_path(2, "bash-5.2.15");
    SnapshotDocument::new(system.clone(), StoreSnapshot {
      closure:  vec![
        StorePathInfo::new(system, Size::from_bytes(1)),
        StorePathInfo::new(bash.clone(), Size::from_bytes(5_000_000)),
      ],
      selected: vec![bash],
    })
  }

  fn to_json(document: &SnapshotDocument) -> String {
    serde_json::to_string(document).unwrap()
  }

  #[test]
  fn document_round_trips_through_json() {
    let document = document();
    assert_eq!(
      SnapshotDocument::from_json(&to_json(&document)).unwrap(),
      document
    );
  }

  #[test]
  fn document_json_has_stable_shape() {
    let value: serde_json::Value =
      serde_json::from_str(&to_json(&document())).unwrap();

    assert_eq!(
      value,
      serde_json::json!({
        "version": SNAPSHOT_FORMAT_VERSION,
        "path": "/nix/store/00000000000000000000000000000001-nixos-system-25.11",
        "closure": [
          {
            "path": "/nix/store/00000000000000000000000000000001-nixos-system-25.11",
            "narSize": 1,
          },
          {
            "path": "/nix/store/00000000000000000000000000000002-bash-5.2.15",
            "narSize": 5_000_000,
          },
        ],
        "selected": ["/nix/store/00000000000000000000000000000002-bash-5.2.15"],
      })
    );
  }

  #[test]
  fn from_json_rejects_unknown_version_before_parsing_fields() {
    let error = SnapshotDocument::from_json(r#"{"version": 2, "other": []}"#)
      .unwrap_err()
      .to_string();
    assert!(error.contains("unsupported dix snapshot format version 2"));
  }

  #[test]
  fn from_json_rejects_non_snapshot_input() {
    let error = SnapshotDocument::from_json("dix: command not found")
      .unwrap_err()
      .to_string();
    assert!(error.contains("not a dix snapshot document"));
  }

  #[test]
  fn from_json_rejects_paths_outside_the_store() {
    let json = to_json(&document()).replace("/nix/store/", "/etc/");
    assert!(SnapshotDocument::from_json(&json).is_err());
  }
}
