use std::num::NonZeroUsize;

use crate::diff::Version;

#[derive(Debug, Eq, PartialEq)]
pub struct Package {
  pub name:    String,
  pub version: Version,
}

impl Package {
  #[must_use]
  pub fn new(name: impl Into<String>, version: impl Into<Version>) -> Self {
    Self {
      name:    name.into(),
      version: version.into(),
    }
  }
}

#[derive(Debug, Eq, PartialEq)]
pub struct VersionChange {
  pub name:                 String,
  pub versions:             Vec<VersionDiff>,
  pub status:               DiffStatus,
  pub has_omitted_versions: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VersionAmount {
  pub version: Version,
  pub amount:  NonZeroUsize,
}

impl VersionAmount {
  #[must_use]
  pub fn new(version: impl Into<Version>, amount: NonZeroUsize) -> Self {
    Self {
      version: version.into(),
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
    version:    Version,
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
