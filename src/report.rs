use std::collections::{
  BTreeMap,
  BTreeSet,
  HashSet,
};

use size::Size;

use crate::{
  DiffStatus,
  Version,
  VersionDiff,
  diff::{
    Package,
    VersionChange,
    diff_packages,
  },
  snapshot::{
    StorePathInfo,
    StoreSnapshot,
  },
};

const PACKAGE_SIZE_DELTA_SIGNIFICANCE_THRESHOLD_BYTES: i64 = 8 * 1024;

#[derive(Debug)]
pub struct DiffReport {
  pub diffs:      Vec<PackageDiff>,
  pub path_stats: PathStats,
  pub size_old:   Size,
  pub size_new:   Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStats {
  pub old:     usize,
  pub new:     usize,
  pub added:   usize,
  pub removed: usize,
}

#[derive(Debug)]
pub struct PackageDiff {
  pub name:      String,
  /// How the package's versions changed, or `None` if only its size did.
  pub change:    Option<VersionChange>,
  pub selection: DerivationSelectionStatus,
  pub size:      PackageSizeDelta,
}

/// The most significant kind of change a package went through.
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
  /// A version was added, removed or changed.
  Version,
  /// Only the amount of some versions changed.
  Amount,
  /// Only the size of the package changed.
  Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageSizeDelta {
  pub old: Size,
  pub new: Size,
}

impl PackageDiff {
  /// Classifies the diff by its most significant change.
  #[must_use]
  pub fn kind(&self) -> ChangeKind {
    let Some(change) = &self.change else {
      return ChangeKind::Size;
    };
    if change
      .versions
      .iter()
      .all(|diff| matches!(diff, VersionDiff::AmountChanged { .. }))
    {
      ChangeKind::Amount
    } else {
      ChangeKind::Version
    }
  }

  /// The status of the version change. Size-only diffs count as changed.
  #[must_use]
  pub fn status(&self) -> DiffStatus {
    self
      .change
      .as_ref()
      .map_or(DiffStatus::Changed, |change| change.status)
  }
}

impl PackageSizeDelta {
  #[must_use]
  pub fn delta(self) -> Size {
    self.new - self.old
  }

  #[must_use]
  pub(crate) fn is_significant(self) -> bool {
    self.delta().bytes().abs() > PACKAGE_SIZE_DELTA_SIGNIFICANCE_THRESHOLD_BYTES
  }

  fn between(
    name: &str,
    old: &SnapshotPackages,
    new: &SnapshotPackages,
  ) -> Self {
    let size_of = |packages: &SnapshotPackages| {
      packages
        .sizes
        .get(name)
        .copied()
        .unwrap_or_else(|| Size::from_bytes(0))
    };
    Self {
      old: size_of(old),
      new: size_of(new),
    }
  }
}

/// Documents if the derivation is a system package and if it was added or
/// removed as such.
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, serde::Serialize,
)]
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
  fn between(
    name: &str,
    old: &SnapshotPackages,
    new: &SnapshotPackages,
  ) -> Self {
    match (old.selected.contains(name), new.selected.contains(name)) {
      (true, true) => Self::Selected,
      (true, false) => Self::NewlyUnselected,
      (false, true) => Self::NewlySelected,
      (false, false) => Self::Unselected,
    }
  }
}

fn package_diffs_with_sizes(
  old: &SnapshotPackages,
  new: &SnapshotPackages,
) -> Vec<PackageDiff> {
  let mut changes = diff_packages(&old.packages, &new.packages);
  let names = changes
    .keys()
    .chain(old.sizes.keys())
    .chain(new.sizes.keys())
    .cloned()
    .collect::<BTreeSet<_>>();

  names
    .into_iter()
    .filter_map(|name| {
      let change = changes.remove(&name);
      let size = PackageSizeDelta::between(&name, old, new);
      (change.is_some() || size.is_significant()).then(|| {
        PackageDiff {
          selection: DerivationSelectionStatus::between(&name, old, new),
          name,
          change,
          size,
        }
      })
    })
    .collect()
}

impl PathStats {
  #[must_use]
  fn between(old: &[StorePathInfo], new: &[StorePathInfo]) -> Self {
    let old_paths = old.iter().map(|info| &info.path).collect::<HashSet<_>>();
    let new_paths = new.iter().map(|info| &info.path).collect::<HashSet<_>>();

    Self {
      old:     old_paths.len(),
      new:     new_paths.len(),
      added:   new_paths.difference(&old_paths).count(),
      removed: old_paths.difference(&new_paths).count(),
    }
  }
}

fn total_nar_size(paths: &[StorePathInfo]) -> Size {
  Size::from_bytes(paths.iter().map(|path| path.nar_size.bytes()).sum::<i64>())
}

fn add_package_size(
  package_sizes: &mut BTreeMap<String, Size>,
  name: &str,
  nar_size: Size,
) {
  package_sizes
    .entry(name.to_owned())
    .and_modify(|size| {
      *size = Size::from_bytes(size.bytes() + nar_size.bytes());
    })
    .or_insert(nar_size);
}

/// The packages of one side of a diff, keyed by package name.
struct SnapshotPackages {
  packages: Vec<Package>,
  selected: HashSet<String>,
  sizes:    BTreeMap<String, Size>,
}

impl SnapshotPackages {
  fn new(snapshot: &StoreSnapshot) -> Self {
    let mut packages = Vec::new();
    let mut sizes = BTreeMap::<String, Size>::new();

    for info in &snapshot.closure {
      let (name, version) = info.path.name_and_version();
      add_package_size(&mut sizes, name, info.nar_size);
      packages.push(Package {
        name:    name.to_owned(),
        version: version.map(Version::new),
      });
    }

    Self {
      packages,
      selected: snapshot
        .selected
        .iter()
        .map(|path| path.name_and_version().0.to_owned())
        .collect(),
      sizes,
    }
  }
}

/// Builds a diff report from two already queried store snapshots.
#[must_use]
pub fn diff_store_snapshots(
  old: &StoreSnapshot,
  new: &StoreSnapshot,
) -> DiffReport {
  let report = DiffReport {
    diffs:      package_diffs_with_sizes(
      &SnapshotPackages::new(old),
      &SnapshotPackages::new(new),
    ),
    path_stats: PathStats::between(&old.closure, &new.closure),
    size_old:   total_nar_size(&old.closure),
    size_new:   total_nar_size(&new.closure),
  };

  tracing::info!(
    diff_count = report.diffs.len(),
    paths_old = report.path_stats.old,
    paths_new = report.path_stats.new,
    paths_added = report.path_stats.added,
    paths_removed = report.path_stats.removed,
    size_old = %report.size_old,
    size_new = %report.size_new,
    "diff report complete"
  );

  report
}

#[cfg(test)]
mod tests {

  use super::*;
  use crate::StorePath;

  fn store_path_with_hash(hash: &str, name: &str) -> StorePath {
    StorePath::try_from(format!("/nix/store/{hash}-{name}"))
      .unwrap_or_else(|err| panic!("failed to create test store path: {err}"))
  }

  fn store_path(name: &str) -> StorePath {
    store_path_with_hash("0123456789abcdefghijklmnopqrstuv", name)
  }

  fn store_path_info(name: &str, nar_size: i64) -> StorePathInfo {
    StorePathInfo {
      path:     store_path(name),
      nar_size: Size::from_bytes(nar_size),
    }
  }

  fn store_path_info_with_hash(
    hash: &str,
    name: &str,
    nar_size: i64,
  ) -> StorePathInfo {
    StorePathInfo {
      path:     store_path_with_hash(hash, name),
      nar_size: Size::from_bytes(nar_size),
    }
  }

  #[test]
  fn path_stats_count_exact_unique_path_sets() {
    let shared = store_path_info("shared-1.0", 1);
    let removed_a = store_path_info("removed-a-1.0", 1);
    let removed_b = store_path_info("removed-b-1.0", 1);
    let added = store_path_info("added-1.0", 1);

    let old = vec![shared.clone(), shared.clone(), removed_a, removed_b];
    let new = vec![shared, added];

    assert_eq!(PathStats::between(&old, &new), PathStats {
      old:     3,
      new:     2,
      added:   1,
      removed: 2,
    });
  }

  #[test]
  fn between_matches_alpha_leading_git_hash_versions() {
    let old_hash = "0bf8387987c21bf2f8ed41d2575a8f22b139687f";
    let new_hash = "cd1931314beafeebc957964c65802961e283411e";
    let old_info =
      store_path_info(&format!("helix-tree-sitter-pod-{old_hash}"), 100);
    let new_info =
      store_path_info(&format!("helix-tree-sitter-pod-{new_hash}"), 200);
    let old_snapshot = StoreSnapshot {
      closure:  vec![old_info],
      selected: Vec::new(),
    };
    let new_snapshot = StoreSnapshot {
      closure:  vec![new_info],
      selected: Vec::new(),
    };
    let report = diff_store_snapshots(&old_snapshot, &new_snapshot);

    assert_eq!(report.diffs.len(), 1);
    let diff = &report.diffs[0];
    assert_eq!(diff.name, "helix-tree-sitter-pod");
    let change = diff.change.as_ref().unwrap();
    assert_eq!(change.status, DiffStatus::Changed);
    assert_eq!(change.versions.len(), 1);
    match &change.versions[0] {
      VersionDiff::Changed { old, new } => {
        assert_eq!(old.version, Some(Version::new(old_hash)));
        assert_eq!(new.version, Some(Version::new(new_hash)));
      },
      other => panic!("expected changed version diff, got {other:?}"),
    }
    assert_eq!(diff.size.old, Size::from_bytes(100));
    assert_eq!(diff.size.new, Size::from_bytes(200));
  }

  #[test]
  fn includes_size_only_package_diff_above_threshold() {
    let old_info = store_path_info_with_hash(
      "0123456789abcdefghijklmnopqrstuv",
      "source",
      100,
    );
    let new_info = store_path_info_with_hash(
      "vutsrqponmlkjihgfedcba9876543210",
      "source",
      9_000,
    );
    let old_snapshot = StoreSnapshot {
      closure:  vec![old_info],
      selected: Vec::new(),
    };
    let new_snapshot = StoreSnapshot {
      closure:  vec![new_info],
      selected: Vec::new(),
    };
    let report = diff_store_snapshots(&old_snapshot, &new_snapshot);

    assert_eq!(report.diffs.len(), 1);
    let diff = &report.diffs[0];
    assert_eq!(diff.name, "source");
    assert_eq!(diff.status(), DiffStatus::Changed);
    assert!(diff.change.is_none());
    assert_eq!(diff.size.old, Size::from_bytes(100));
    assert_eq!(diff.size.new, Size::from_bytes(9_000));
  }

  #[test]
  fn diff_store_snapshots_preserves_selection_status() {
    let old_info = store_path_info("package-1.0", 100);
    let new_info = store_path_info("package-2.0", 200);
    let old_snapshot = StoreSnapshot {
      closure:  vec![old_info.clone()],
      selected: vec![old_info.path],
    };
    let new_snapshot = StoreSnapshot {
      closure:  vec![new_info],
      selected: Vec::new(),
    };
    let report = diff_store_snapshots(&old_snapshot, &new_snapshot);

    assert_eq!(report.diffs.len(), 1);
    assert_eq!(
      report.diffs[0].selection,
      DerivationSelectionStatus::NewlyUnselected,
    );
  }
}
