use std::num::NonZeroUsize;

use crate::Version;

#[derive(Debug, Eq, PartialEq)]
pub struct Diff {
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

  #[must_use]
  pub fn try_new(version: impl Into<Version>, amount: usize) -> Option<Self> {
    Some(Self::new(version, NonZeroUsize::new(amount)?))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffStatus {
  Changed,
  Mixed,
  Upgraded,
  Downgraded,
  Added,
  Removed,
}
