use std::{
  cmp,
  collections::HashSet,
};

#[cfg(feature = "json")] use serde::Serialize;

use crate::CountedVersion;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub struct Diff<T = Vec<CountedVersion>> {
  pub name:                String,
  pub old:                 T,
  pub new:                 T,
  pub status:              DiffStatus,
  pub selection:           DerivationSelectionStatus,
  pub has_common_versions: bool,
}

impl<T> Default for Diff<T>
where
  T: Default,
{
  fn default() -> Self {
    Self {
      name:                String::default(),
      old:                 T::default(),
      new:                 T::default(),
      status:              DiffStatus::Changed(Change::UpgradeDowngrade),
      selection:           DerivationSelectionStatus::Unselected,
      has_common_versions: false,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum Change {
  UpgradeDowngrade,
  Upgraded,
  Downgraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum DiffStatus {
  Changed(Change),
  Added,
  Removed,
}

impl PartialOrd for DiffStatus {
  fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl cmp::Ord for DiffStatus {
  fn cmp(&self, other: &Self) -> cmp::Ordering {
    #[expect(clippy::pattern_type_mismatch, clippy::match_same_arms)]
    match (self, other) {
      (Self::Changed(_), Self::Changed(_)) => cmp::Ordering::Equal,
      (Self::Added, Self::Added) => cmp::Ordering::Equal,
      (Self::Removed, Self::Removed) => cmp::Ordering::Equal,
      (Self::Changed(_), _) => cmp::Ordering::Less,
      (_, Self::Changed(_)) => cmp::Ordering::Greater,
      (Self::Added, Self::Removed) => cmp::Ordering::Less,
      (Self::Removed, Self::Added) => cmp::Ordering::Greater,
    }
  }
}

/// Documents if the derivation is a system package and if
/// it was added / removed as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum DerivationSelectionStatus {
  /// The derivation is a system package, status unchanged.
  Selected,
  /// The derivation was not a system package before but is now.
  NewlySelected,
  /// The derivation is and was a dependency.
  Unselected,
  /// The derivation was a system package before but is not anymore.
  NewlyUnselected,
}

impl DerivationSelectionStatus {
  pub(crate) fn from_names(
    name: &str,
    old: &HashSet<String>,
    new: &HashSet<String>,
  ) -> Self {
    match (old.contains(name), new.contains(name)) {
      (true, true) => Self::Selected,
      (true, false) => Self::NewlyUnselected,
      (false, true) => Self::NewlySelected,
      (false, false) => Self::Unselected,
    }
  }
}
