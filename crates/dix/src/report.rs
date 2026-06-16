use std::{
  collections::{
    BTreeMap,
    HashSet,
  },
  path::Path,
};

use dix_diff::{
  Diff as EngineDiff,
  Package,
  PackageSnapshot,
  diff_snapshots,
};
use eyre::Result;
use size::Size;

use crate::{
  DiffStatus,
  StorePath,
  Version,
  VersionDiff,
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
  diffs:      Vec<PackageDiff>,
  path_stats: PathStats,
  size_old:   Size,
  size_new:   Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStats {
  old:     usize,
  new:     usize,
  added:   usize,
  removed: usize,
}

#[derive(Debug)]
pub struct PackageDiff {
  pub name:                 String,
  pub versions:             Vec<VersionDiff>,
  pub status:               DiffStatus,
  pub selection:            DerivationSelectionStatus,
  pub has_omitted_versions: bool,
  pub size:                 PackageSizeDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageSizeDelta {
  old: Size,
  new: Size,
}

impl PackageDiff {
  fn from_engine(
    diff: EngineDiff,
    selected_old: &HashSet<String>,
    selected_new: &HashSet<String>,
    old_sizes: &BTreeMap<String, Size>,
    new_sizes: &BTreeMap<String, Size>,
  ) -> Self {
    let name = diff.name;
    Self {
      selection: DerivationSelectionStatus::from_names(
        &name,
        selected_old,
        selected_new,
      ),
      size: PackageSizeDelta::between(&name, old_sizes, new_sizes),
      name,
      versions: diff.versions,
      status: diff.status,
      has_omitted_versions: diff.has_omitted_versions,
    }
  }
}

impl PackageSizeDelta {
  #[must_use]
  pub const fn new(old: Size, new: Size) -> Self {
    Self { old, new }
  }

  #[must_use]
  pub const fn old_size(self) -> Size {
    self.old
  }

  #[must_use]
  pub const fn new_size(self) -> Size {
    self.new
  }

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
    old_sizes: &BTreeMap<String, Size>,
    new_sizes: &BTreeMap<String, Size>,
  ) -> Self {
    Self {
      old: old_sizes
        .get(name)
        .copied()
        .unwrap_or_else(|| Size::from_bytes(0)),
      new: new_sizes
        .get(name)
        .copied()
        .unwrap_or_else(|| Size::from_bytes(0)),
    }
  }
}

/// Documents if the derivation is a system package and if it was added or
/// removed as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
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
  fn from_names(
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

impl DiffReport {
  #[must_use]
  pub fn diffs(&self) -> &[PackageDiff] {
    &self.diffs
  }

  #[must_use]
  pub const fn path_stats(&self) -> PathStats {
    self.path_stats
  }

  #[must_use]
  pub const fn size_old(&self) -> Size {
    self.size_old
  }

  #[must_use]
  pub const fn size_new(&self) -> Size {
    self.size_new
  }

  #[cfg(all(test, feature = "json"))]
  pub(crate) const fn new_for_test(
    diffs: Vec<PackageDiff>,
    path_stats: PathStats,
    size_old: Size,
    size_new: Size,
  ) -> Self {
    Self {
      diffs,
      path_stats,
      size_old,
      size_new,
    }
  }
}

fn package_diffs_with_sizes(
  old: &SnapshotParts,
  new: &SnapshotParts,
) -> Vec<PackageDiff> {
  let mut diffs = diff_snapshots(&old.snapshot, &new.snapshot)
    .into_iter()
    .map(|diff| {
      let name = diff.name.clone();
      (
        name,
        PackageDiff::from_engine(
          diff,
          &old.selected,
          &new.selected,
          &old.package_sizes,
          &new.package_sizes,
        ),
      )
    })
    .collect::<BTreeMap<_, _>>();

  for name in old.package_sizes.keys().chain(new.package_sizes.keys()) {
    if diffs.contains_key(name) {
      continue;
    }

    let size =
      PackageSizeDelta::between(name, &old.package_sizes, &new.package_sizes);
    if !size.is_significant() {
      continue;
    }

    diffs.insert(name.clone(), PackageDiff {
      name: name.clone(),
      versions: Vec::new(),
      status: DiffStatus::Changed,
      selection: DerivationSelectionStatus::from_names(
        name,
        &old.selected,
        &new.selected,
      ),
      has_omitted_versions: false,
      size,
    });
  }

  diffs.into_values().collect()
}

impl PathStats {
  #[must_use]
  pub const fn old_count(self) -> usize {
    self.old
  }

  #[must_use]
  pub const fn new_count(self) -> usize {
    self.new
  }

  #[must_use]
  pub const fn added_count(self) -> usize {
    self.added
  }

  #[must_use]
  pub const fn removed_count(self) -> usize {
    self.removed
  }

  #[cfg(test)]
  pub(crate) const fn new_for_test(
    old: usize,
    new: usize,
    added: usize,
    removed: usize,
  ) -> Self {
    Self {
      old,
      new,
      added,
      removed,
    }
  }

  #[cfg(test)]
  #[must_use]
  fn between(old: &[StorePath], new: &[StorePath]) -> Self {
    let old_paths = old.iter().collect::<HashSet<_>>();
    let new_paths = new.iter().collect::<HashSet<_>>();

    Self {
      old:     old_paths.len(),
      new:     new_paths.len(),
      added:   new_paths.difference(&old_paths).count(),
      removed: old_paths.difference(&new_paths).count(),
    }
  }

  #[must_use]
  fn between_path_info(old: &[StorePathInfo], new: &[StorePathInfo]) -> Self {
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

struct SnapshotPackages {
  packages:       Vec<Package>,
  selected_names: HashSet<String>,
  package_sizes:  BTreeMap<String, Size>,
}

impl SnapshotPackages {
  fn from_store_snapshot(
    snapshot: &StoreSnapshot,
    context: SnapshotContext,
  ) -> Self {
    let mut packages = Vec::new();
    let mut package_sizes = BTreeMap::<String, Size>::new();

    for info in &snapshot.closure {
      let Some(parsed) =
        parse_store_path_lossy(info.path(), context.dependencies)
      else {
        continue;
      };
      add_package_size(&mut package_sizes, &parsed.name, info.nar_size());
      packages.push(Package::new(
        parsed.name,
        parsed
          .version
          .map_or_else(|| "<none>".into(), Version::from),
      ));
    }

    Self {
      packages,
      selected_names: snapshot
        .selected
        .iter()
        .filter_map(|path| parse_store_path_lossy(path, context.selected))
        .map(|parsed| parsed.name)
        .collect(),
      package_sizes,
    }
  }

  fn into_snapshot_parts(self) -> SnapshotParts {
    SnapshotParts {
      snapshot:      PackageSnapshot::new(self.packages),
      selected:      self.selected_names,
      package_sizes: self.package_sizes,
    }
  }
}

struct SnapshotParts {
  snapshot:      PackageSnapshot,
  selected:      HashSet<String>,
  package_sizes: BTreeMap<String, Size>,
}

#[derive(Clone, Copy)]
struct SnapshotContext {
  dependencies: &'static str,
  selected:     &'static str,
}

struct ParsedStorePath {
  name:    String,
  version: Option<String>,
}

/// Queries Nix store data and builds a pure diff report.
///
/// # Errors
///
/// Returns an error if store connection or package querying fails.
pub fn query_diff_report(
  path_old: &Path,
  path_new: &Path,
  force_correctness: bool,
) -> Result<DiffReport> {
  tracing::debug!(
    old_path = %path_old.display(),
    new_path = %path_new.display(),
    force_correctness = force_correctness,
    "starting diff report computation"
  );

  let (old, new) = CombinedStoreBackend::query_with_correctness(
    force_correctness,
    |backend| {
      Ok((
        query_store_snapshot_with_backend(backend, path_old)?,
        query_store_snapshot_with_backend(backend, path_new)?,
      ))
    },
  )?;
  let report = diff_store_snapshots(&old, &new);
  let path_stats = report.path_stats();

  tracing::info!(
    diff_count = report.diffs().len(),
    paths_old = path_stats.old_count(),
    paths_new = path_stats.new_count(),
    paths_added = path_stats.added_count(),
    paths_removed = path_stats.removed_count(),
    size_old = %report.size_old(),
    size_new = %report.size_new(),
    "diff report complete"
  );

  Ok(report)
}

/// Builds a diff report from two already queried store snapshots.
#[must_use]
pub fn diff_store_snapshots(
  old: &StoreSnapshot,
  new: &StoreSnapshot,
) -> DiffReport {
  let path_stats = PathStats::between_path_info(&old.closure, &new.closure);
  let size_old = total_nar_size(&old.closure);
  let size_new = total_nar_size(&new.closure);
  let old = SnapshotPackages::from_store_snapshot(old, SnapshotContext {
    dependencies: "old dependency",
    selected:     "old system",
  })
  .into_snapshot_parts();
  let new = SnapshotPackages::from_store_snapshot(new, SnapshotContext {
    dependencies: "new dependency",
    selected:     "new system",
  })
  .into_snapshot_parts();

  DiffReport {
    diffs: package_diffs_with_sizes(&old, &new),
    path_stats,
    size_old,
    size_new,
  }
}

fn parse_store_path_lossy(
  path: &StorePath,
  context: &str,
) -> Option<ParsedStorePath> {
  match path.parse_name_and_version() {
    Ok((name, version)) => {
      Some(ParsedStorePath {
        name:    name.to_owned(),
        version: version.map(str::to_owned),
      })
    },
    Err(error) => {
      tracing::warn!(path = %path.display(), "dropping unparsable {context} path: {error}");
      None
    },
  }
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
  fn path_stats_count_exact_unique_path_sets() {
    let shared = store_path("shared-1.0");
    let removed_a = store_path("removed-a-1.0");
    let removed_b = store_path("removed-b-1.0");
    let added = store_path("added-1.0");

    let old = vec![shared.clone(), shared.clone(), removed_a, removed_b];
    let new = vec![shared, added];

    assert_eq!(
      PathStats::between(&old, &new),
      PathStats::new_for_test(3, 2, 1, 2),
    );
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
    assert_eq!(diff.status, DiffStatus::Changed);
    assert_eq!(diff.versions.len(), 1);
    match &diff.versions[0] {
      VersionDiff::Changed { old, new } => {
        assert_eq!(old.version.name, old_hash);
        assert_eq!(new.version.name, new_hash);
      },
      other => panic!("expected changed version diff, got {other:?}"),
    }
    assert_eq!(diff.size.old_size(), Size::from_bytes(100));
    assert_eq!(diff.size.new_size(), Size::from_bytes(200));
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
    assert_eq!(diff.status, DiffStatus::Changed);
    assert!(diff.versions.is_empty());
    assert_eq!(diff.size.old_size(), Size::from_bytes(100));
    assert_eq!(diff.size.new_size(), Size::from_bytes(9_000));
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
