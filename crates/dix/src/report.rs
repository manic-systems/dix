use std::{
  collections::HashSet,
  path::{
    Path,
    PathBuf,
  },
  thread,
};

use dix_diff::{
  Diff as EngineDiff,
  Package,
  PackageSnapshot,
  diff_snapshots,
};
use eyre::{
  Result,
  WrapErr as _,
  eyre,
};
use size::Size;

use crate::{
  DiffStatus,
  StorePath,
  Version,
  VersionDiff,
  store::{
    CombinedStoreBackend,
    StoreBackend,
  },
};

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
}

impl PackageDiff {
  fn from_engine(
    diff: EngineDiff,
    selected_old: &HashSet<String>,
    selected_new: &HashSet<String>,
  ) -> Self {
    Self {
      selection:            DerivationSelectionStatus::from_names(
        &diff.name,
        selected_old,
        selected_new,
      ),
      name:                 diff.name,
      versions:             diff.versions,
      status:               diff.status,
      has_omitted_versions: diff.has_omitted_versions,
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

  #[cfg(test)]
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

  #[must_use]
  fn from_queried_snapshots(
    snapshots: QueriedSnapshots,
    size_old: Size,
    size_new: Size,
  ) -> Self {
    let QueriedSnapshots {
      old,
      new,
      path_stats,
    } = snapshots;
    let (old_snapshot, selected_old) = old.into_snapshot_parts();
    let (new_snapshot, selected_new) = new.into_snapshot_parts();

    Self {
      diffs: diff_snapshots(&old_snapshot, &new_snapshot)
        .into_iter()
        .map(|diff| {
          PackageDiff::from_engine(diff, &selected_old, &selected_new)
        })
        .collect(),
      path_stats,
      size_old,
      size_new,
    }
  }
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
}

struct SnapshotPackages {
  packages:       Vec<Package>,
  selected_names: HashSet<String>,
}

struct QueriedSnapshots {
  old:        SnapshotPackages,
  new:        SnapshotPackages,
  path_stats: PathStats,
}

impl SnapshotPackages {
  fn from_store_paths(
    dependencies: impl IntoIterator<Item = StorePath>,
    selected: impl IntoIterator<Item = StorePath>,
    context: SnapshotContext,
  ) -> Self {
    Self {
      packages:       dependencies
        .into_iter()
        .filter_map(|path| package_from_store_path(&path, context.dependencies))
        .collect(),
      selected_names: selected
        .into_iter()
        .filter_map(|path| parse_store_path_lossy(&path, context.selected))
        .map(|parsed| parsed.name)
        .collect(),
    }
  }

  fn into_snapshot_parts(self) -> (PackageSnapshot, HashSet<String>) {
    (PackageSnapshot::new(self.packages), self.selected_names)
  }
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
/// Returns an error if store connection, package querying, closure-size
/// querying, or the background size worker fails.
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

  let size_handle = spawn_closure_sizes(
    path_old.to_path_buf(),
    path_new.to_path_buf(),
    force_correctness,
  );
  let snapshots = CombinedStoreBackend::query_with_correctness(
    force_correctness,
    |backend| query_report_snapshots(backend, path_old, path_new),
  );
  let sizes = join_closure_sizes(size_handle);

  let snapshots = snapshots?;
  let (size_old, size_new) = sizes?;
  let report =
    DiffReport::from_queried_snapshots(snapshots, size_old, size_new);
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

fn query_report_snapshots(
  backend: &impl StoreBackend,
  path_old: &Path,
  path_new: &Path,
) -> Result<QueriedSnapshots> {
  tracing::debug!("querying dependencies for old path");
  let paths_old = backend.query_dependents(path_old).with_context(|| {
    format!("failed to query dependencies of '{}'", path_old.display())
  })?;

  tracing::debug!("querying dependencies for new path");
  let paths_new = backend.query_dependents(path_new).with_context(|| {
    format!("failed to query dependencies of '{}'", path_new.display())
  })?;

  tracing::debug!("querying system derivations for old path");
  let system_derivations_old = backend
    .query_system_derivations(path_old)
    .with_context(|| {
      format!(
        "failed to query system derivations of '{}'",
        path_old.display()
      )
    })?;

  tracing::debug!("querying system derivations for new path");
  let system_derivations_new = backend
    .query_system_derivations(path_new)
    .with_context(|| {
      format!(
        "failed to query system derivations of '{}'",
        path_new.display()
      )
    })?;

  let path_stats = PathStats::between(&paths_old, &paths_new);

  Ok(QueriedSnapshots {
    old: SnapshotPackages::from_store_paths(
      paths_old,
      system_derivations_old,
      SnapshotContext {
        dependencies: "old dependency",
        selected:     "old system",
      },
    ),
    new: SnapshotPackages::from_store_paths(
      paths_new,
      system_derivations_new,
      SnapshotContext {
        dependencies: "new dependency",
        selected:     "new system",
      },
    ),
    path_stats,
  })
}

fn package_from_store_path(path: &StorePath, context: &str) -> Option<Package> {
  parse_store_path_lossy(path, context).map(|parsed| {
    Package::new(
      parsed.name,
      parsed
        .version
        .map_or_else(|| "<none>".into(), Version::from),
    )
  })
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

fn spawn_closure_sizes(
  path_old: PathBuf,
  path_new: PathBuf,
  force_correctness: bool,
) -> thread::JoinHandle<Result<(Size, Size)>> {
  tracing::debug!("calculating closure sizes in background");

  thread::spawn(move || {
    CombinedStoreBackend::query_with_correctness(force_correctness, |backend| {
      Ok((
        backend.query_closure_size(&path_old)?,
        backend.query_closure_size(&path_new)?,
      ))
    })
  })
}

fn join_closure_sizes(
  handle: thread::JoinHandle<Result<(Size, Size)>>,
) -> Result<(Size, Size)> {
  handle.join().map_err(|_| {
    tracing::error!("closure size thread panicked");
    eyre!("failed to get closure size due to thread error")
  })?
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::*;

  fn store_path(name: &str) -> StorePath {
    StorePath::try_from(PathBuf::from(format!(
      "/nix/store/0123456789abcdefghijklmnopqrstuv-{name}"
    )))
    .unwrap_or_else(|err| panic!("failed to create test store path: {err}"))
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
}
