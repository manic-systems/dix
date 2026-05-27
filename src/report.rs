use std::{
  collections::HashSet,
  path::{
    Path,
    PathBuf,
  },
  thread,
};

use dix_diff::{
  DiffReport,
  Package,
  PackageSnapshot,
  Version,
};
use eyre::{
  Result,
  WrapErr as _,
  eyre,
};
use size::Size;

use crate::{
  StorePath,
  store::{
    CombinedStoreBackend,
    StoreBackend,
  },
};

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

  fn into_snapshot(self, closure_size: Size) -> PackageSnapshot {
    PackageSnapshot::new(self.packages, self.selected_names, closure_size)
  }
}

#[derive(Clone, Copy)]
struct SnapshotContext {
  dependencies: &'static str,
  selected:     &'static str,
}

struct ParsedStorePath {
  name:    String,
  version: Option<Version>,
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
  let report = DiffReport::between(
    old_packages.into_snapshot(size_old),
    new_packages.into_snapshot(size_new),
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
      parsed.version.unwrap_or_else(|| "<none>".into()),
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
        name: name.to_owned(),
        version,
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
