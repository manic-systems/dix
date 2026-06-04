use std::{
  cmp,
  collections::BTreeMap,
  num::NonZeroUsize,
};

use itertools::EitherOrBoth;

use crate::{
  Version,
  matching::match_version_amounts,
  model::{
    Diff,
    DiffStatus,
    VersionAmount,
    VersionDiff,
  },
  snapshot::{
    Package,
    PackageSnapshot,
  },
  version::VersionChangeOrdering,
};

#[must_use]
pub fn diff_snapshots(
  old: &PackageSnapshot,
  new: &PackageSnapshot,
) -> Vec<Diff> {
  let mut diffs = build_package_diffs(&old.packages, &new.packages);
  canonicalize_diffs(&mut diffs);
  diffs
}

fn build_package_diffs<'a>(
  packages_old: impl IntoIterator<Item = &'a Package>,
  packages_new: impl IntoIterator<Item = &'a Package>,
) -> Vec<Diff> {
  let versions_by_name = collect_package_versions(packages_old, packages_new);
  generate_diffs_from_version_map(versions_by_name)
}

fn canonicalize_diffs(diffs: &mut [Diff]) {
  diffs.sort_by(|left, right| left.name.cmp(&right.name));
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

struct VersionChanges {
  diffs:                Vec<VersionDiff>,
  has_omitted_versions: bool,
}

fn collect_version_changes(
  old_versions: &VersionAmounts,
  new_versions: &VersionAmounts,
) -> VersionChanges {
  let mut old_only = Vec::new();
  let mut new_only = Vec::new();
  let mut amount_diffs = Vec::new();
  let mut has_omitted_versions = false;

  for (version, old_amount) in old_versions.iter() {
    match new_versions.get(version) {
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

  for (version, new_amount) in new_versions.iter() {
    if !old_versions.contains(version) {
      new_only.push(VersionAmount::new(version.clone(), new_amount));
    }
  }

  let mut diffs = version_diffs(&old_only, &new_only);
  diffs.extend(amount_diffs);

  VersionChanges {
    diffs,
    has_omitted_versions,
  }
}

fn generate_diffs_from_version_map(
  packages: BTreeMap<String, PackageVersions>,
) -> Vec<Diff> {
  let mut result = Vec::with_capacity(packages.len());

  for (name, package_versions) in packages {
    let version_changes =
      collect_version_changes(&package_versions.old, &package_versions.new);

    let status = if version_changes.diffs.is_empty() {
      continue;
    } else if package_versions.old.is_empty() {
      DiffStatus::Added
    } else if package_versions.new.is_empty() {
      DiffStatus::Removed
    } else {
      determine_change_status(&version_changes.diffs)
        .unwrap_or(DiffStatus::Changed)
    };

    result.push(Diff {
      name,
      versions: version_changes.diffs,
      status,
      has_omitted_versions: version_changes.has_omitted_versions,
    });
  }

  result
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

fn determine_change_status(
  version_diffs: &[VersionDiff],
) -> Option<DiffStatus> {
  let mut saw_upgrade = false;
  let mut saw_downgrade = false;
  let mut saw_changed = false;

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
          | VersionChangeOrdering::Unordered => {
            saw_changed = true;
          },
        }
      },
      VersionDiff::AmountChanged { .. } => {
        saw_changed = true;
      },
    }
    if saw_upgrade && saw_downgrade {
      break;
    }
  }

  match (saw_upgrade, saw_downgrade, saw_changed) {
    (true, true, _) => Some(DiffStatus::Mixed),
    (true, false, _) => Some(DiffStatus::Upgraded),
    (false, true, _) => Some(DiffStatus::Downgraded),
    (false, false, true) => Some(DiffStatus::Changed),
    (false, false, false) => None,
  }
}

#[cfg(test)]
mod tests {
  use std::{
    collections::{
      BTreeMap,
      HashSet,
    },
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

  fn diff_for(name: &str, old: &[&str], new: &[&str]) -> Diff {
    let mut diffs =
      generate_diffs_from_version_map(version_map(name, old, new));
    canonicalize_diffs(&mut diffs);
    assert_eq!(diffs.len(), 1);
    diffs.remove(0)
  }

  fn old_side_count(diff: &Diff) -> usize {
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

  fn new_side_count(diff: &Diff) -> usize {
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
    assert!(generate_diffs_from_version_map(packages).is_empty());
  }

  #[test]
  fn generate_diffs_unchanged_package() {
    assert!(
      generate_diffs_from_version_map(version_map("package", &["1.0.0"], &[
        "1.0.0"
      ]))
      .is_empty()
    );
  }

  #[test]
  fn generate_diffs_unchanged_duplicate_versions() {
    assert!(
      generate_diffs_from_version_map(version_map(
        "package",
        &["1.0.0", "1.0.0"],
        &["1.0.0", "1.0.0"],
      ))
      .is_empty()
    );
  }

  #[test]
  fn generate_diffs_added_package() {
    let diff = diff_for("new-pkg", &[], &["1.0.0"]);

    assert_eq!(diff.name, "new-pkg");
    assert_eq!(diff.status, DiffStatus::Added);
    assert_eq!(diff.versions, vec![VersionDiff::Added(version_amount(
      "1.0.0", 1
    ))]);
  }

  #[test]
  fn generate_diffs_removed_package() {
    let diff = diff_for("old-pkg", &["1.0.0"], &[]);

    assert_eq!(diff.name, "old-pkg");
    assert_eq!(diff.status, DiffStatus::Removed);
    assert_eq!(diff.versions, vec![VersionDiff::Removed(version_amount(
      "1.0.0", 1
    ))]);
  }

  #[test]
  fn generate_diffs_reports_added_version_amounts() {
    let diff = diff_for("new-pkg", &[], &["1.0.0", "1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Added);
    assert_eq!(diff.versions, vec![VersionDiff::Added(version_amount(
      "1.0.0", 2
    ))]);
  }

  #[test]
  fn generate_diffs_reports_removed_version_amounts() {
    let diff = diff_for("old-pkg", &["1.0.0", "1.0.0"], &[]);

    assert_eq!(diff.status, DiffStatus::Removed);
    assert_eq!(diff.versions, vec![VersionDiff::Removed(version_amount(
      "1.0.0", 2
    ))]);
  }

  #[test]
  fn generate_diffs_reports_changed_version_amounts() {
    let diff = diff_for("pkg", &["1.0.0"], &["1.0.0", "1.0.0"]);

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
    let diff = diff_for("pkg", &["1.0.0"], &["2.0.0", "2.0.0"]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("1.0.0", 1),
      new: version_amount("2.0.0", 2),
    }]);
  }

  #[test]
  fn generate_diffs_upgraded() {
    let diff = diff_for("pkg", &["1.0.0"], &["2.0.0"]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("1.0.0", 1),
      new: version_amount("2.0.0", 1),
    }]);
  }

  #[test]
  fn generate_diffs_reports_git_hash_only_change_as_changed() {
    let diff = diff_for("sunsetr", &["0.11.1-946aa34"], &["0.11.1-3564204"]);

    assert_eq!(diff.status, DiffStatus::Changed);
    assert_eq!(diff.versions, vec![VersionDiff::Changed {
      old: version_amount("0.11.1-946aa34", 1),
      new: version_amount("0.11.1-3564204", 1),
    }]);
  }

  #[test]
  fn generate_diffs_uses_dated_component_before_git_hash_for_upgrade() {
    let diff = diff_for("yazi", &["25.05.31pre20250531_946aa34"], &[
      "25.05.31pre20250601_3564204",
    ]);

    assert_eq!(diff.status, DiffStatus::Upgraded);
  }

  #[test]
  fn generate_diffs_downgraded() {
    let diff = diff_for("pkg", &["2.0.0"], &["1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Downgraded);
  }

  #[test]
  fn generate_diffs_upgrade_downgrade() {
    let diff = diff_for("pkg", &["1.0", "5.0"], &["2.0", "4.0"]);

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

    let result = generate_diffs_from_version_map(packages);
    let names: HashSet<_> =
      result.iter().map(|diff| diff.name.as_str()).collect();

    assert_eq!(result.len(), 2);
    assert!(names.contains("pkg-a"));
    assert!(names.contains("pkg-b"));
    assert!(!names.contains("pkg-c"));
  }

  #[test]
  fn generate_diffs_common_versions() {
    let diff = diff_for("pkg", &["1.0.0", "2.0.0"], &["2.0.0", "3.0.0"]);

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

    let result = generate_diffs_from_version_map(packages);

    assert_eq!(result.len(), 1);
    assert!(result[0].has_omitted_versions);
    assert_eq!(old_side_count(&result[0]), 50);
    assert_eq!(new_side_count(&result[0]), 50);
  }
}
