use std::{
  cmp,
  collections::BTreeMap,
  num::NonZeroUsize,
};

use itertools::EitherOrBoth;

use crate::diff::{
  Version,
  matching::match_version_amounts,
  model::{
    DiffStatus,
    Package,
    VersionAmount,
    VersionChange,
    VersionDiff,
  },
  version::VersionChangeOrdering,
};

/// Diffs the packages of two closures.
///
/// # Returns
///
/// The change of every package whose versions changed, keyed by name.
#[must_use]
pub fn diff_packages(
  old: &[Package],
  new: &[Package],
) -> BTreeMap<String, VersionChange> {
  version_changes(collect_package_versions(old, new))
}

fn collect_package_versions<'a>(
  old: impl IntoIterator<Item = &'a Package>,
  new: impl IntoIterator<Item = &'a Package>,
) -> BTreeMap<String, PackageVersions> {
  let mut packages = BTreeMap::<String, PackageVersions>::new();
  let mut old_count = 0usize;
  let mut new_count = 0usize;

  for package in old {
    old_count += 1;
    packages
      .entry(package.name.clone())
      .or_default()
      .old
      .add(package.version.clone());
  }

  for package in new {
    new_count += 1;
    packages
      .entry(package.name.clone())
      .or_default()
      .new
      .add(package.version.clone());
  }

  debug_assert_eq!(
    old_count + new_count,
    packages
      .values()
      .map(PackageVersions::total_amount)
      .sum::<usize>(),
  );

  packages
}

#[derive(Default)]
struct PackageVersions {
  old: VersionAmounts,
  new: VersionAmounts,
}

impl PackageVersions {
  fn total_amount(&self) -> usize {
    self.old.total_amount() + self.new.total_amount()
  }
}

#[derive(Default)]
struct VersionAmounts {
  amounts: BTreeMap<Version, NonZeroUsize>,
}

impl VersionAmounts {
  fn add(&mut self, version: Version) {
    self
      .amounts
      .entry(version)
      .and_modify(|amount| *amount = increment_amount(*amount))
      .or_insert(NonZeroUsize::MIN);
  }

  fn is_empty(&self) -> bool {
    self.amounts.is_empty()
  }

  fn get(&self, version: &Version) -> Option<NonZeroUsize> {
    self.amounts.get(version).copied()
  }

  fn contains(&self, version: &Version) -> bool {
    self.amounts.contains_key(version)
  }

  fn iter(&self) -> impl Iterator<Item = (&Version, NonZeroUsize)> {
    self
      .amounts
      .iter()
      .map(|(version, amount)| (version, *amount))
  }

  fn total_amount(&self) -> usize {
    self.amounts.values().map(|amount| amount.get()).sum()
  }
}

fn increment_amount(amount: NonZeroUsize) -> NonZeroUsize {
  amount
    .get()
    .checked_add(1)
    .and_then(NonZeroUsize::new)
    .unwrap_or_else(|| panic!("version amount overflowed usize"))
}

fn version_changes(
  packages: BTreeMap<String, PackageVersions>,
) -> BTreeMap<String, VersionChange> {
  packages
    .into_iter()
    .filter_map(|(name, versions)| Some((name, version_change(&versions)?)))
    .collect()
}

/// Compares the versions of one package.
///
/// # Returns
///
/// The change, or `None` if every version kept its amount.
fn version_change(versions: &PackageVersions) -> Option<VersionChange> {
  let mut old_only = Vec::new();
  let mut new_only = Vec::new();
  let mut amount_diffs = Vec::new();
  let mut has_omitted_versions = false;

  for (version, old_amount) in versions.old.iter() {
    match versions.new.get(version) {
      Some(new_amount) if old_amount == new_amount => {
        has_omitted_versions = true;
      },
      Some(new_amount) => {
        amount_diffs.push(VersionDiff::AmountChanged {
          version: version.clone(),
          old_amount,
          new_amount,
        });
      },
      None => old_only.push(VersionAmount::new(version.clone(), old_amount)),
    }
  }

  for (version, new_amount) in versions.new.iter() {
    if !versions.old.contains(version) {
      new_only.push(VersionAmount::new(version.clone(), new_amount));
    }
  }

  let mut diffs = version_diffs(&old_only, &new_only);
  diffs.extend(amount_diffs);
  if diffs.is_empty() {
    return None;
  }

  let status = if versions.old.is_empty() {
    DiffStatus::Added
  } else if versions.new.is_empty() {
    DiffStatus::Removed
  } else {
    change_status(&diffs)
  };

  Some(VersionChange {
    versions: diffs,
    status,
    has_omitted_versions,
  })
}

fn version_diffs(
  old_versions: &[VersionAmount],
  new_versions: &[VersionAmount],
) -> Vec<VersionDiff> {
  match_version_amounts(old_versions, new_versions)
    .into_iter()
    .map(|ver_diff| {
      match ver_diff {
        EitherOrBoth::Left(old) => VersionDiff::Removed(old.clone()),
        EitherOrBoth::Right(new) => VersionDiff::Added(new.clone()),
        EitherOrBoth::Both(old, new) => {
          VersionDiff::Changed {
            old: old.clone(),
            new: new.clone(),
          }
        },
      }
    })
    .collect()
}

/// Classifies the version diffs of a package that exists on both sides.
fn change_status(version_diffs: &[VersionDiff]) -> DiffStatus {
  let mut saw_upgrade = false;
  let mut saw_downgrade = false;

  for ver_diff in version_diffs {
    match ver_diff {
      VersionDiff::Removed(_) => saw_downgrade = true,
      VersionDiff::Added(_) => saw_upgrade = true,
      VersionDiff::Changed { old, new } => {
        match old.version.change_ordering(&new.version) {
          VersionChangeOrdering::Ordered(cmp::Ordering::Less) => {
            saw_upgrade = true;
          },
          VersionChangeOrdering::Ordered(cmp::Ordering::Greater) => {
            saw_downgrade = true;
          },
          VersionChangeOrdering::Ordered(cmp::Ordering::Equal)
          | VersionChangeOrdering::Unordered => {},
        }
      },
      VersionDiff::AmountChanged { .. } => {},
    }
    if saw_upgrade && saw_downgrade {
      break;
    }
  }

  match (saw_upgrade, saw_downgrade) {
    (true, true) => DiffStatus::Mixed,
    (true, false) => DiffStatus::Upgraded,
    (false, true) => DiffStatus::Downgraded,
    (false, false) => DiffStatus::Changed,
  }
}

#[cfg(test)]
mod tests {
  use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
  };

  use super::*;

  fn nonzero(amount: usize) -> NonZeroUsize {
    NonZeroUsize::new(amount)
      .unwrap_or_else(|| panic!("test version amount must be nonzero"))
  }

  fn version_amount(version: &str, amount: usize) -> VersionAmount {
    VersionAmount::new(version, nonzero(amount))
  }

  fn package_versions(old: &[&str], new: &[&str]) -> PackageVersions {
    let mut versions = PackageVersions::default();
    for version in old {
      versions.old.add(Version::new(*version));
    }
    for version in new {
      versions.new.add(Version::new(*version));
    }
    versions
  }

  fn version_map(
    name: &str,
    old: &[&str],
    new: &[&str],
  ) -> BTreeMap<String, PackageVersions> {
    let mut packages = BTreeMap::new();
    packages.insert(name.to_owned(), package_versions(old, new));
    packages
  }

  fn change_for(name: &str, old: &[&str], new: &[&str]) -> VersionChange {
    let mut changes = version_changes(version_map(name, old, new));
    assert_eq!(changes.len(), 1);
    changes
      .remove(name)
      .unwrap_or_else(|| panic!("expected a change for {name}"))
  }

  fn old_side_count(diff: &VersionChange) -> usize {
    diff
      .versions
      .iter()
      .filter(|version_diff| {
        matches!(
          version_diff,
          VersionDiff::Removed(_)
            | VersionDiff::Changed { .. }
            | VersionDiff::AmountChanged { .. }
        )
      })
      .count()
  }

  fn new_side_count(diff: &VersionChange) -> usize {
    diff
      .versions
      .iter()
      .filter(|version_diff| {
        matches!(
          version_diff,
          VersionDiff::Added(_)
            | VersionDiff::Changed { .. }
            | VersionDiff::AmountChanged { .. }
        )
      })
      .count()
  }

  #[test]
  fn generate_diffs_empty_paths() {
    let packages = BTreeMap::new();
    assert!(version_changes(packages).is_empty());
  }

  #[test]
  fn generate_diffs_unchanged_package() {
    assert!(
      version_changes(version_map("package", &["1.0.0"], &["1.0.0"]))
        .is_empty()
    );
  }

  #[test]
  fn generate_diffs_unchanged_duplicate_versions() {
    assert!(
      version_changes(version_map("package", &["1.0.0", "1.0.0"], &[
        "1.0.0", "1.0.0"
      ],))
      .is_empty()
    );
  }

  #[test]
  fn generate_diffs_added_package() {
    let diff = change_for("new-pkg", &[], &["1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Added);
    assert_eq!(diff.versions, vec![VersionDiff::Added(version_amount(
      "1.0.0", 1
    ))]);
  }

  #[test]
  fn generate_diffs_removed_package() {
    let diff = change_for("old-pkg", &["1.0.0"], &[]);

    assert_eq!(diff.status, DiffStatus::Removed);
    assert_eq!(diff.versions, vec![VersionDiff::Removed(version_amount(
      "1.0.0", 1
    ))]);
  }

  #[test]
  fn generate_diffs_reports_added_version_amounts() {
    let diff = change_for("new-pkg", &[], &["1.0.0", "1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Added);
    assert_eq!(diff.versions, vec![VersionDiff::Added(version_amount(
      "1.0.0", 2
    ))]);
  }

  #[test]
  fn generate_diffs_reports_removed_version_amounts() {
    let diff = change_for("old-pkg", &["1.0.0", "1.0.0"], &[]);

    assert_eq!(diff.status, DiffStatus::Removed);
    assert_eq!(diff.versions, vec![VersionDiff::Removed(version_amount(
      "1.0.0", 2
    ))]);
  }

  #[test]
  fn generate_diffs_reports_changed_version_amounts() {
    let diff = change_for("pkg", &["1.0.0"], &["1.0.0", "1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Changed);
    assert!(!diff.has_omitted_versions);
    assert_eq!(diff.versions, vec![VersionDiff::AmountChanged {
      version:    Version::new("1.0.0"),
      old_amount: nonzero(1),
      new_amount: nonzero(2),
    }]);
  }

  #[test]
  fn generate_diffs_reports_version_change_amounts() {
    let diff = change_for("pkg", &["1.0.0"], &["2.0.0", "2.0.0"]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("1.0.0", 1),
      new: version_amount("2.0.0", 2),
    }]);
  }

  #[test]
  fn generate_diffs_upgraded() {
    let diff = change_for("pkg", &["1.0.0"], &["2.0.0"]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("1.0.0", 1),
      new: version_amount("2.0.0", 1),
    }]);
  }

  #[test]
  fn generate_diffs_reports_git_hash_only_change_as_changed() {
    let diff = change_for("sunsetr", &["0.11.1-946aa34"], &["0.11.1-3564204"]);

    assert_eq!(diff.status, DiffStatus::Changed);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("0.11.1-946aa34", 1),
      new: version_amount("0.11.1-3564204", 1),
    }]);
  }

  #[test]
  fn generate_diffs_uses_dated_component_before_git_hash_for_upgrade() {
    let diff = change_for("yazi", &["25.05.31pre20250531_946aa34"], &[
      "25.05.31pre20250601_3564204",
    ]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
  }

  #[test]
  fn generate_diffs_downgraded() {
    let diff = change_for("pkg", &["2.0.0"], &["1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Downgraded);
  }

  #[test]
  fn generate_diffs_upgrade_downgrade() {
    let diff = change_for("pkg", &["1.0", "5.0"], &["2.0", "4.0"]);

    assert_eq!(diff.status, DiffStatus::Mixed);
  }

  #[test]
  fn generate_diffs_multiple_packages() {
    let mut packages = BTreeMap::new();
    packages
      .insert("pkg-a".to_owned(), package_versions(&["1.0.0"], &["2.0.0"]));
    packages.insert("pkg-b".to_owned(), package_versions(&[], &["1.0.0"]));
    packages
      .insert("pkg-c".to_owned(), package_versions(&["1.0.0"], &["1.0.0"]));

    let result = version_changes(packages);
    assert_eq!(result.keys().collect::<Vec<_>>(), ["pkg-a", "pkg-b"]);
  }

  #[test]
  fn generate_diffs_common_versions() {
    let diff = change_for("pkg", &["1.0.0", "2.0.0"], &["2.0.0", "3.0.0"]);

    assert!(diff.has_omitted_versions);
    assert_eq!(diff.status, DiffStatus::Upgraded);
  }

  #[test]
  fn generate_diffs_many_versions() {
    let old_versions: Vec<_> = (0..100)
      .map(|index| Version::new(format!("1.{index}.{index}")))
      .collect();
    let new_versions: Vec<_> = (50..150)
      .map(|index| Version::new(format!("1.{index}.{index}")))
      .collect();
    let mut versions = PackageVersions::default();
    for version in old_versions {
      versions.old.add(version);
    }
    for version in new_versions {
      versions.new.add(version);
    }
    let mut packages = BTreeMap::new();
    packages.insert("large-pkg".to_owned(), versions);

    let result = version_changes(packages);

    assert_eq!(result.len(), 1);
    let change = &result["large-pkg"];
    assert!(change.has_omitted_versions);
    assert_eq!(old_side_count(change), 50);
    assert_eq!(new_side_count(change), 50);
  }
}
