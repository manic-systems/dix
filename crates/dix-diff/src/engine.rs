use std::{
  cmp,
  collections::{
    HashMap,
    HashSet,
  },
  hash::BuildHasher,
};

use itertools::EitherOrBoth;

use crate::{
  Version,
  matching::match_version_lists,
  model::{
    Change,
    DerivationSelectionStatus,
    Diff,
    DiffStatus,
  },
  snapshot::{
    Package,
    PackageSnapshot,
  },
};

pub(crate) fn diff_snapshots(
  old: &PackageSnapshot,
  new: &PackageSnapshot,
) -> Vec<Diff> {
  build_package_diffs(
    &old.packages,
    &new.packages,
    &old.selected_names,
    &new.selected_names,
  )
}

fn build_package_diffs<'a>(
  packages_old: impl IntoIterator<Item = &'a Package>,
  packages_new: impl IntoIterator<Item = &'a Package>,
  selected_old: &HashSet<String>,
  selected_new: &HashSet<String>,
) -> Vec<Diff> {
  let versions_by_name = collect_package_versions(packages_old, packages_new);
  let mut diffs = generate_diffs_from_version_map(versions_by_name);
  add_selection_status(&mut diffs, selected_old, selected_new);

  diffs
}

pub(crate) fn canonicalize_diffs(diffs: &mut [Diff]) {
  for diff in diffs.iter_mut() {
    diff.new.sort();
    diff.old.sort();
  }
  diffs.sort();
}

fn collect_package_versions<'a>(
  old: impl IntoIterator<Item = &'a Package>,
  new: impl IntoIterator<Item = &'a Package>,
) -> HashMap<String, (Vec<Version>, Vec<Version>)> {
  let mut packages: HashMap<String, (Vec<Version>, Vec<Version>)> =
    HashMap::new();
  let mut old_count = 0usize;
  let mut new_count = 0usize;

  for package in old {
    old_count += 1;
    tracing::trace!(name = %package.name, version = ?package.version, "collected old package");
    packages
      .entry(package.name.clone())
      .or_default()
      .0
      .push(package.version.clone());
  }

  for package in new {
    new_count += 1;
    tracing::trace!(name = %package.name, version = ?package.version, "collected new package");
    packages
      .entry(package.name.clone())
      .or_default()
      .1
      .push(package.version.clone());
  }

  tracing::debug!(
    old_count = old_count,
    new_count = new_count,
    unique_packages = packages.len(),
    "collected packages"
  );

  packages
}

fn count_versions(versions: Vec<Version>) -> HashMap<Version, usize> {
  let mut counts = HashMap::new();
  for version in versions {
    *counts.entry(version).or_insert(0) += 1;
  }
  counts
}

fn generate_diffs_from_version_map<S: BuildHasher>(
  packages: HashMap<String, (Vec<Version>, Vec<Version>), S>,
) -> Vec<Diff> {
  let mut result = Vec::with_capacity(packages.len());

  #[expect(clippy::iter_over_hash_type)]
  for (name, (old_versions, new_versions)) in packages {
    let old_counts = count_versions(old_versions);
    let new_counts = count_versions(new_versions);

    let old_set: HashSet<Version> = old_counts.keys().cloned().collect();
    let new_set: HashSet<Version> = new_counts.keys().cloned().collect();

    let common_count = old_set.intersection(&new_set).count();

    let unique_old: Vec<Version> =
      old_set.difference(&new_set).cloned().collect();
    let unique_new: Vec<Version> =
      new_set.difference(&old_set).cloned().collect();

    let status = if unique_old.is_empty() && unique_new.is_empty() {
      continue;
    } else if common_count == 0 && unique_old.is_empty() {
      DiffStatus::Added
    } else if common_count == 0 && unique_new.is_empty() {
      DiffStatus::Removed
    } else if unique_old.is_empty() || unique_new.is_empty() {
      DiffStatus::Changed(Change::UpgradeDowngrade)
    } else {
      determine_change_status(&unique_old, &unique_new)
        .unwrap_or(DiffStatus::Changed(Change::UpgradeDowngrade))
    };

    result.push(Diff {
      name,
      old: unique_old,
      new: unique_new,
      status,
      selection: DerivationSelectionStatus::Unselected,
      has_common_versions: common_count > 0,
    });
  }

  result
}

fn determine_change_status(
  old_versions: &[Version],
  new_versions: &[Version],
) -> Option<DiffStatus> {
  let mut saw_upgrade = false;
  let mut saw_downgrade = false;

  for ver_diff in match_version_lists(old_versions, new_versions) {
    match ver_diff {
      EitherOrBoth::Left(_) => saw_downgrade = true,
      EitherOrBoth::Right(_) => saw_upgrade = true,
      EitherOrBoth::Both(old, new) => {
        match old.cmp(new) {
          cmp::Ordering::Less => saw_upgrade = true,
          cmp::Ordering::Greater => saw_downgrade = true,
          cmp::Ordering::Equal => {},
        }
      },
    }
    if saw_upgrade && saw_downgrade {
      break;
    }
  }

  match (saw_upgrade, saw_downgrade) {
    (true, true) => Some(DiffStatus::Changed(Change::UpgradeDowngrade)),
    (true, false) => Some(DiffStatus::Changed(Change::Upgraded)),
    (false, true) => Some(DiffStatus::Changed(Change::Downgraded)),
    (false, false) => None,
  }
}

fn add_selection_status(
  diffs: &mut [Diff],
  system_paths_old: &HashSet<String>,
  system_paths_new: &HashSet<String>,
) {
  for diff in diffs {
    diff.selection = DerivationSelectionStatus::from_names(
      &diff.name,
      system_paths_old,
      system_paths_new,
    );
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn version_map(
    name: &str,
    old: &[&str],
    new: &[&str],
  ) -> HashMap<String, (Vec<Version>, Vec<Version>)> {
    let mut packages = HashMap::new();
    packages.insert(
      name.to_owned(),
      (
        old.iter().copied().map(Version::new).collect(),
        new.iter().copied().map(Version::new).collect(),
      ),
    );
    packages
  }

  fn diff_for(name: &str, old: &[&str], new: &[&str]) -> Diff {
    let mut diffs =
      generate_diffs_from_version_map(version_map(name, old, new));
    canonicalize_diffs(&mut diffs);
    diffs.pop().expect("expected exactly one diff")
  }

  #[test]
  fn generate_diffs_empty_paths() {
    let packages: HashMap<String, (Vec<Version>, Vec<Version>)> =
      HashMap::new();
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
  fn generate_diffs_added_package() {
    let diff = diff_for("new-pkg", &[], &["1.0.0"]);

    assert_eq!(diff.name, "new-pkg");
    assert_eq!(diff.status, DiffStatus::Added);
    assert!(diff.old.is_empty());
    assert_eq!(diff.new, vec![Version::new("1.0.0")]);
  }

  #[test]
  fn generate_diffs_removed_package() {
    let diff = diff_for("old-pkg", &["1.0.0"], &[]);

    assert_eq!(diff.name, "old-pkg");
    assert_eq!(diff.status, DiffStatus::Removed);
    assert_eq!(diff.old, vec![Version::new("1.0.0")]);
    assert!(diff.new.is_empty());
  }

  #[test]
  fn generate_diffs_upgraded() {
    let diff = diff_for("pkg", &["1.0.0"], &["2.0.0"]);

    assert_eq!(diff.status, DiffStatus::Changed(Change::Upgraded));
    assert_eq!(diff.old, vec![Version::new("1.0.0")]);
    assert_eq!(diff.new, vec![Version::new("2.0.0")]);
  }

  #[test]
  fn generate_diffs_downgraded() {
    let diff = diff_for("pkg", &["2.0.0"], &["1.0.0"]);

    assert_eq!(diff.status, DiffStatus::Changed(Change::Downgraded));
  }

  #[test]
  fn generate_diffs_upgrade_downgrade() {
    let diff = diff_for("pkg", &["1.0", "5.0"], &["2.0", "4.0"]);

    assert_eq!(diff.status, DiffStatus::Changed(Change::UpgradeDowngrade));
  }

  #[test]
  fn generate_diffs_multiple_packages() {
    let mut packages = HashMap::new();
    packages.insert(
      "pkg-a".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );
    packages.insert("pkg-b".to_owned(), (vec![], vec![Version::new("1.0.0")]));
    packages.insert(
      "pkg-c".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("1.0.0")]),
    );

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

    assert!(diff.has_common_versions);
    assert_eq!(diff.status, DiffStatus::Changed(Change::Upgraded));
  }

  #[test]
  fn generate_diffs_many_versions() {
    let old_versions: Vec<_> = (0..100)
      .map(|index| Version::new(format!("1.{index}.{index}")))
      .collect();
    let new_versions: Vec<_> = (50..150)
      .map(|index| Version::new(format!("1.{index}.{index}")))
      .collect();
    let mut packages = HashMap::new();
    packages.insert("large-pkg".to_owned(), (old_versions, new_versions));

    let result = generate_diffs_from_version_map(packages);

    assert_eq!(result.len(), 1);
    assert!(result[0].has_common_versions);
    assert_eq!(result[0].old.len(), 50);
    assert_eq!(result[0].new.len(), 50);
  }

  #[test]
  fn add_selection_status_marks_all_transitions() {
    let mut diffs = vec![
      Diff {
        name: "selected".to_owned(),
        ..Diff::default()
      },
      Diff {
        name: "newly-selected".to_owned(),
        ..Diff::default()
      },
      Diff {
        name: "newly-unselected".to_owned(),
        ..Diff::default()
      },
      Diff {
        name: "unselected".to_owned(),
        ..Diff::default()
      },
    ];
    let old =
      HashSet::from(["selected".to_owned(), "newly-unselected".to_owned()]);
    let new =
      HashSet::from(["selected".to_owned(), "newly-selected".to_owned()]);

    add_selection_status(&mut diffs, &old, &new);

    assert_eq!(diffs[0].selection, DerivationSelectionStatus::Selected);
    assert_eq!(diffs[1].selection, DerivationSelectionStatus::NewlySelected);
    assert_eq!(
      diffs[2].selection,
      DerivationSelectionStatus::NewlyUnselected
    );
    assert_eq!(diffs[3].selection, DerivationSelectionStatus::Unselected);
  }
}
