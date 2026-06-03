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
  pub diffs:    Vec<PackageDiff>,
  pub size_old: Size,
  pub size_new: Size,
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
  pub fn between(
    old: &PackageSnapshot,
    new: &PackageSnapshot,
    selected_old: &HashSet<String>,
    selected_new: &HashSet<String>,
    size_old: Size,
    size_new: Size,
  ) -> Self {
    Self {
      diffs: diff_snapshots(old, new)
        .into_iter()
        .map(|diff| PackageDiff::from_engine(diff, selected_old, selected_new))
        .collect(),
      size_old,
      size_new,
    }
  }
}

struct SnapshotPackages {
  packages:       Vec<Package>,
  selected_names: HashSet<String>,
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
  let packages = CombinedStoreBackend::query_with_correctness(
    force_correctness,
    |backend| query_snapshot_packages(backend, path_old, path_new),
  );
  let sizes = join_closure_sizes(size_handle);

  let (old_packages, new_packages) = packages?;
  let (size_old, size_new) = sizes?;
  let (old_snapshot, selected_old) = old_packages.into_snapshot_parts();
  let (new_snapshot, selected_new) = new_packages.into_snapshot_parts();
  let report = DiffReport::between(
    &old_snapshot,
    &new_snapshot,
    &selected_old,
    &selected_new,
    size_old,
    size_new,
  );

  tracing::info!(diff_count = report.diffs.len(), size_old = %report.size_old, size_new = %report.size_new, "diff report complete");

  Ok(report)
}

fn query_snapshot_packages(
  backend: &impl StoreBackend,
  path_old: &Path,
  path_new: &Path,
) -> Result<(SnapshotPackages, SnapshotPackages)> {
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

  Ok((
    SnapshotPackages::from_store_paths(
      paths_old,
      system_derivations_old,
      SnapshotContext {
        dependencies: "old dependency",
        selected:     "old system",
      },
    ),
    SnapshotPackages::from_store_paths(
      paths_new,
      system_derivations_new,
      SnapshotContext {
        dependencies: "new dependency",
        selected:     "new system",
      },
    ),
  ))
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
