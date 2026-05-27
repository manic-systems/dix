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
};
use eyre::{
  Result,
  eyre,
};

use crate::{
  StorePath,
  store::{
    CombinedStoreBackend,
    StoreBackend as _,
    StorePathSnapshot,
  },
};

/// Queries Nix store data and builds a pure diff report.
///
/// # Errors
///
/// Returns an error if store connection, snapshot querying, or the background
/// snapshot worker fails.
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

  let old_snapshot_handle =
    spawn_store_path_snapshot(path_old.to_path_buf(), force_correctness);
  let new_snapshot = query_store_path_snapshot(path_new, force_correctness);
  let old_snapshot = join_store_path_snapshot(old_snapshot_handle);

  let old_snapshot = package_snapshot_from_store(old_snapshot?);
  let new_snapshot = package_snapshot_from_store(new_snapshot?);
  let report = DiffReport::between(old_snapshot, new_snapshot);

  tracing::info!(diff_count = report.diffs.len(), size_old = %report.size_old, size_new = %report.size_new, "diff report complete");

  Ok(report)
}

fn query_store_path_snapshot(
  path: &Path,
  force_correctness: bool,
) -> Result<StorePathSnapshot> {
  tracing::debug!(path = %path.display(), "querying store path snapshot");
  CombinedStoreBackend::query_with_correctness(force_correctness, |backend| {
    backend.query_path_snapshot(path)
  })
}

fn package_snapshot_from_store(snapshot: StorePathSnapshot) -> PackageSnapshot {
  let packages: Vec<Package> = snapshot
    .dependencies
    .iter()
    .map(package_from_store_path)
    .collect();
  let selected_names = snapshot
    .selected
    .into_iter()
    .map(|path| path.package_name().to_owned())
    .collect::<HashSet<_>>();

  PackageSnapshot::new(packages, selected_names, snapshot.closure_size)
}

fn package_from_store_path(path: &StorePath) -> Package {
  let version = path
    .package_version()
    .cloned()
    .unwrap_or_else(|| "<none>".into());
  Package::new(path.package_name().to_owned(), version)
}

fn spawn_store_path_snapshot(
  path: PathBuf,
  force_correctness: bool,
) -> thread::JoinHandle<Result<StorePathSnapshot>> {
  thread::spawn(move || query_store_path_snapshot(&path, force_correctness))
}

fn join_store_path_snapshot(
  handle: thread::JoinHandle<Result<StorePathSnapshot>>,
) -> Result<StorePathSnapshot> {
  handle.join().map_err(|_| {
    tracing::error!("store path snapshot thread panicked");
    eyre!("failed to get store path snapshot due to thread error")
  })?
}
