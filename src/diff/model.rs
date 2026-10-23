use std::num::NonZeroUsize;

use crate::diff::Version;

#[derive(Debug, Eq, PartialEq)]
pub struct Package {
  pub name:    String,
  /// `None` for packages without a version, such as flake sources.
  pub version: Option<Version>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct VersionChange {
  pub versions:             Vec<VersionDiff>,
  pub status:               DiffStatus,
  pub has_omitted_versions: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VersionAmount {
  /// `None` for packages without a version, such as flake sources.
  pub version: Option<Version>,
  pub amount:  NonZeroUsize,
}

impl VersionAmount {
  /// Creates an amount of a present `version`.
  #[must_use]
  pub fn new(version: impl Into<Version>, amount: NonZeroUsize) -> Self {
    Self {
      version: Some(version.into()),
      amount,
    }
  }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum VersionDiff {
  Removed(VersionAmount),
  Added(VersionAmount),
  Changed {
    old: VersionAmount,
    new: VersionAmount,
  },
  AmountChanged {
    version:    Option<Version>,
    old_amount: NonZeroUsize,
    new_amount: NonZeroUsize,
  },
}

#[derive(
  Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize,
)]
pub enum DiffStatus {
  Changed,
  Mixed,
  Upgraded,
  Downgraded,
  Added,
  Removed,
}
