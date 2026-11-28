use std::{
  error,
  fmt,
};
#[cfg(feature = "store")]
use std::{
  io,
  path::PathBuf,
  process::ExitStatus,
  string::FromUtf8Error,
};

#[cfg(feature = "store")] use crate::StorePath;

/// The errors returned by the public `dix` API.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
  /// A path is not of the form `/nix/store/<hash>-<name>`.
  InvalidStorePath(String),
  /// The input is not a valid dix snapshot document.
  InvalidSnapshot(serde_json::Error),
  /// The snapshot document has a format version this `dix` does not support.
  ///
  /// The supported version is
  /// [`SnapshotDocument::FORMAT_VERSION`](crate::SnapshotDocument::FORMAT_VERSION).
  UnsupportedSnapshotVersion(u64),
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
    source: Box<Self>,
  },
  /// Querying the packages a store path selects failed.
  #[cfg(feature = "store")]
  SelectionQuery {
    path:   StorePath,
    source: Box<Self>,
  },
  /// A Nix command cannot be run.
  #[cfg(feature = "store")]
  RunCommand { command: String, source: io::Error },
  /// A Nix command exited unsuccessfully.
  #[cfg(feature = "store")]
  CommandFailed {
    command: String,
    status:  ExitStatus,
    stderr:  String,
  },
  /// A Nix command printed invalid UTF-8.
  #[cfg(feature = "store")]
  CommandNotUtf8 {
    command: String,
    source:  FromUtf8Error,
  },
  /// A Nix command printed a line `dix` cannot parse.
  #[cfg(feature = "store")]
  UnexpectedOutput { command: String, line: String },
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
      Self::InvalidSnapshot(_) => write!(f, "invalid dix snapshot document"),
      Self::UnsupportedSnapshotVersion(version) => {
        write!(
          f,
          "unsupported dix snapshot format version {version} (supported: {})",
          crate::SnapshotDocument::FORMAT_VERSION
        )
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
      #[cfg(feature = "store")]
      Self::RunCommand { command, .. } => {
        write!(f, "failed to run {command}")
      },
      #[cfg(feature = "store")]
      Self::CommandFailed {
        command,
        status,
        stderr,
      } => {
        write!(f, "{command} exited with {status}")?;
        if !stderr.is_empty() {
          write!(f, ": {stderr}")?;
        }
        Ok(())
      },
      #[cfg(feature = "store")]
      Self::CommandNotUtf8 { command, .. } => {
        write!(f, "output of {command} is not valid UTF-8")
      },
      #[cfg(feature = "store")]
      Self::UnexpectedOutput { command, line } => {
        write!(f, "{command} printed an unexpected line: '{line}'")
      },
    }
  }
}

impl error::Error for Error {
  fn source(&self) -> Option<&(dyn error::Error + 'static)> {
    match self {
      Self::InvalidSnapshot(source) => Some(source),
      #[cfg(feature = "store")]
      Self::Canonicalize { source, .. } | Self::RunCommand { source, .. } => {
        Some(source)
      },
      #[cfg(feature = "store")]
      Self::ClosureQuery { source, .. }
      | Self::SelectionQuery { source, .. } => Some(source.as_ref()),
      #[cfg(feature = "store")]
      Self::CommandNotUtf8 { source, .. } => Some(source),
      _ => None,
    }
  }
}
