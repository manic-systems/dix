use std::{
  collections::{
    BTreeMap,
    BTreeSet,
    HashSet,
  },
  path::Path,
};

use misstep::Result;
use size::Size;

use crate::{
  DiffStatus,
  StorePath,
  VersionDiff,
  diff::{
    Package,
    VersionChange,
    diff_packages,
  },
  snapshot::{
    StoreSnapshot,
    query_store_snapshot_with_backend,
  },
  store::{
    CombinedStoreBackend,
    StorePathInfo,
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
  pub fn is_significant(self) -> bool {
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
    let old_paths = old.iter().map(StorePathInfo::path).collect::<HashSet<_>>();
    let new_paths = new.iter().map(StorePathInfo::path).collect::<HashSet<_>>();

    Self {
      old:     old_paths.len(),
      new:     new_paths.len(),
      added:   new_paths.difference(&old_paths).count(),
      removed: old_paths.difference(&new_paths).count(),
    }
  }
}

fn total_nar_size(paths: &[StorePathInfo]) -> Size {
  Size::from_bytes(
    paths
      .iter()
      .map(|path| path.nar_size().bytes())
      .sum::<i64>(),
  )
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
  /// `side` names the snapshot in warnings about unparsable paths.
  fn new(snapshot: &StoreSnapshot, side: &str) -> Self {
    let mut packages = Vec::new();
    let mut sizes = BTreeMap::<String, Size>::new();

    for info in &snapshot.closure {
      let Some((name, version)) = parse_store_path_lossy(info.path(), side)
      else {
        continue;
      };
      add_package_size(&mut sizes, name, info.nar_size());
      packages.push(Package::new(name, version.unwrap_or("<none>")));
    }

    Self {
      packages,
      selected: snapshot
        .selected
        .iter()
        .filter_map(|path| parse_store_path_lossy(path, side))
        .map(|(name, _)| name.to_owned())
        .collect(),
      sizes,
    }
  }
}

/// Queries Nix store data and builds a pure diff report.
///
/// # Errors
///
/// Returns an error if package querying fails.
pub fn query_diff_report(
  path_old: &Path,
  path_new: &Path,
) -> Result<DiffReport> {
  tracing::debug!(
    old_path = %path_old.display(),
    new_path = %path_new.display(),
    "starting diff report computation"
  );

  let backend = CombinedStoreBackend::open();
  Ok(diff_store_snapshots(
    &query_store_snapshot_with_backend(&backend, path_old)?,
    &query_store_snapshot_with_backend(&backend, path_new)?,
  ))
}

/// Queries consecutive paths on one store backend and diffs adjacent
/// snapshots. Each path is queried once, even when it participates in two
/// comparisons.
///
/// # Returns
///
/// One report per adjacent pair of `paths`, in order; empty if there are
/// fewer than two paths.
///
/// # Errors
///
/// Returns an error if querying a path fails.
pub fn query_adjacent_diff_reports(paths: &[&Path]) -> Result<Vec<DiffReport>> {
  if paths.len() < 2 {
    return Ok(Vec::new());
  }

  let backend = CombinedStoreBackend::open();
  adjacent_diff_reports(paths, |path| {
    query_store_snapshot_with_backend(&backend, path)
  })
}

/// Diffs adjacent snapshots of `paths`, obtaining each snapshot from `query`
/// exactly once.
///
/// # Returns
///
/// One report per adjacent pair of `paths`, in order; empty if there are
/// fewer than two paths.
///
/// # Errors
///
/// Returns the first error from `query`.
fn adjacent_diff_reports(
  paths: &[&Path],
  mut query: impl FnMut(&Path) -> Result<StoreSnapshot>,
) -> Result<Vec<DiffReport>> {
  let mut paths = paths.iter();
  let Some(first) = paths.next() else {
    return Ok(Vec::new());
  };
  let mut previous = query(first)?;
  let mut reports = Vec::with_capacity(paths.len());
  for path in paths {
    let next = query(path)?;
    reports.push(diff_store_snapshots(&previous, &next));
    previous = next;
  }
  Ok(reports)
}

/// Builds a diff report from two already queried store snapshots.
#[must_use]
pub fn diff_store_snapshots(
  old: &StoreSnapshot,
  new: &StoreSnapshot,
) -> DiffReport {
  let report = DiffReport {
    diffs:      package_diffs_with_sizes(
      &SnapshotPackages::new(old, "old"),
      &SnapshotPackages::new(new, "new"),
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

fn parse_store_path_lossy<'a>(
  path: &'a StorePath,
  side: &str,
) -> Option<(&'a str, Option<&'a str>)> {
  path
    .parse_name_and_version()
    .inspect_err(|error| {
      tracing::warn!(path = %path.display(), "dropping unparsable {side} path: {error}");
    })
    .ok()
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::*;

  fn store_path_with_hash(hash: &str, name: &str) -> StorePath {
    StorePath::try_from(PathBuf::from(format!("/nix/store/{hash}-{name}")))
      .unwrap_or_else(|err| panic!("failed to create test store path: {err}"))
  }

  fn store_path(name: &str) -> StorePath {
    store_path_with_hash("0123456789abcdefghijklmnopqrstuv", name)
  }

  fn store_path_info(name: &str, nar_size: i64) -> StorePathInfo {
    StorePathInfo::new(store_path(name), Size::from_bytes(nar_size))
  }

  fn store_path_info_with_hash(
    hash: &str,
    name: &str,
    nar_size: i64,
  ) -> StorePathInfo {
    StorePathInfo::new(
      store_path_with_hash(hash, name),
      Size::from_bytes(nar_size),
    )
  }

  #[test]
  fn adjacent_reports_query_each_snapshot_once_and_diff_in_order() {
    let paths = [Path::new("first"), Path::new("second"), Path::new("third")];
    let mut queried = Vec::new();
    let reports = adjacent_diff_reports(&paths, |path| {
      queried.push(path.to_path_buf());
      let (name, size) = match path.to_str().unwrap() {
        "first" => ("package-1", 100),
        "second" => ("package-2", 200),
        "third" => ("package-3", 300),
        other => panic!("unexpected query: {other}"),
      };
      Ok(StoreSnapshot {
        closure:  vec![store_path_info(name, size)],
        selected: Vec::new(),
      })
    })
    .unwrap();

    assert_eq!(queried, paths.map(Path::to_path_buf));
    assert_eq!(reports.len(), 2);
    assert_eq!(
      (reports[0].size_old.bytes(), reports[0].size_new.bytes()),
      (100, 200)
    );
    assert_eq!(
      (reports[1].size_old.bytes(), reports[1].size_new.bytes()),
      (200, 300)
    );
    assert!(reports.iter().all(|report| report.diffs.len() == 1));
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
        assert_eq!(old.version.name, old_hash);
        assert_eq!(new.version.name, new_hash);
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
      selected: vec![old_info.path().clone()],
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
