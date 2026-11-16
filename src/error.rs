use std::{
  error,
  fmt,
};
#[cfg(feature = "store")]
use std::{
  io,
  path::PathBuf,
};

#[cfg(feature = "store")] use crate::StorePath;

/// The errors returned by the public `dix` API.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
  /// A path is not of the form `/nix/store/<hash>-<name>`.
  InvalidStorePath(String),
  /// The input is not a dix snapshot document.
  NotASnapshot(serde_json::Error),
  /// The snapshot document has a format version this `dix` does not support.
  UnsupportedSnapshotVersion(u32),
  /// The snapshot document has a supported version, but invalid contents.
  InvalidSnapshot(serde_json::Error),
  /// A path cannot be canonicalized.
  #[cfg(feature = "store")]
  Canonicalize { path: PathBuf, source: io::Error },
  /// A path resolves to something that is not valid unicode.
  #[cfg(feature = "store")]
  NotUnicode(PathBuf),
  /// A store path is not registered as valid in the Nix store.
  #[cfg(feature = "store")]
  PathNotValid(StorePath),
  /// Querying the closure of a store path failed.
  #[cfg(feature = "store")]
  ClosureQuery {
    path:   StorePath,
    source: Box<dyn error::Error + Send + Sync>,
  },
  /// Querying the packages a store path selects failed.
  #[cfg(feature = "store")]
  SelectionQuery {
    path:   StorePath,
    source: Box<dyn error::Error + Send + Sync>,
  },
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::InvalidStorePath(path) => {
        write!(
          f,
          "path '{path}' is not of the form {}<hash>-<name>",
          crate::STORE_DIR
        )
      },
      Self::NotASnapshot(_) => {
        write!(f, "input is not a dix snapshot document")
      },
      Self::UnsupportedSnapshotVersion(version) => {
        write!(
          f,
          "unsupported dix snapshot format version {version} (supported: {})",
          crate::snapshot::SNAPSHOT_FORMAT_VERSION
        )
      },
      Self::InvalidSnapshot(_) => {
        write!(f, "failed to parse dix snapshot document")
      },
      #[cfg(feature = "store")]
      Self::Canonicalize { path, .. } => {
        write!(f, "failed to canonicalize path '{}'", path.display())
      },
      #[cfg(feature = "store")]
      Self::NotUnicode(path) => {
        write!(f, "path '{}' is not valid unicode", path.display())
      },
      #[cfg(feature = "store")]
      Self::PathNotValid(path) => {
        write!(f, "'{}' is not a valid store path", path.display())
      },
      #[cfg(feature = "store")]
      Self::ClosureQuery { path, .. } => {
        write!(
          f,
          "failed to query closure path info of '{}'",
          path.display()
        )
      },
      #[cfg(feature = "store")]
      Self::SelectionQuery { path, .. } => {
        write!(
          f,
          "failed to query system derivations of '{}'",
          path.display()
        )
      },
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::NotASnapshot(source) | Self::InvalidSnapshot(source) => {
        Some(source)
      },
      #[cfg(feature = "store")]
      Self::Canonicalize { source, .. } => Some(source),
      #[cfg(feature = "store")]
      Self::ClosureQuery { source, .. }
      | Self::SelectionQuery { source, .. } => Some(source.as_ref()),
      _ => None,
    }
  }
}
