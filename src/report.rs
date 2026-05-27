use std::{
  fmt,
  path::{
    Path,
    PathBuf,
  },
  thread,
};

use eyre::{
  Result,
  eyre,
};
use size::Size;
use yansi::Paint as _;

use crate::{
  diff::{
    self,
    Diff,
  },
  store::{
    CombinedStoreBackend,
    StoreBackend as _,
  },
};

#[derive(Debug)]
pub struct DiffReport {
  pub diffs:    Vec<Diff>,
  pub size_old: Size,
  pub size_new: Size,
}

impl DiffReport {
  /// Queries all data required to render a diff report.
  ///
  /// Package and closure-size queries run in parallel so human and JSON output
  /// share one data pipeline without regressing CLI latency.
  ///
  /// # Errors
  ///
  /// Returns an error if store connection, package querying, closure-size
  /// querying, or the background size worker fails.
  pub fn query(
    path_old: &Path,
    path_new: &Path,
    force_correctness: bool,
  ) -> Result<Self> {
    tracing::debug!(
      old_path = %path_old.display(),
      new_path = %path_new.display(),
      force_correctness = force_correctness,
      "starting diff report computation"
    );

    let size_handle = spawn_size_diff(
      path_old.to_path_buf(),
      path_new.to_path_buf(),
      force_correctness,
    );
    let diffs = CombinedStoreBackend::query_with_correctness(
      force_correctness,
      |backend| diff::query_package_diffs(backend, path_old, path_new),
    );
    let sizes = join_size_diff(size_handle);

    let mut diffs = diffs?;
    diff::canonicalize_diffs(&mut diffs);
    let (size_old, size_new) = sizes?;

    tracing::info!(diff_count = diffs.len(), size_old = %size_old, size_new = %size_new, "diff report complete");

    Ok(Self {
      diffs,
      size_old,
      size_new,
    })
  }
}

/// Writes a full diff report to the provided writer.
///
/// # Returns
///
/// Returns the number of package diffs written.
///
/// # Errors
///
/// Returns an error if writing to the output fails.
pub fn write_diff_report(
  writer: &mut impl fmt::Write,
  report: &DiffReport,
) -> Result<usize, fmt::Error> {
  writeln!(writer)?;

  let wrote = diff::render_package_diffs(writer, &report.diffs)?;

  if wrote > 0 {
    writeln!(writer)?;
  }

  write_size_diff(writer, report.size_old, report.size_new)?;

  Ok(wrote)
}

/// Spawns a background task to compute the closure sizes required by the report
/// builder.
fn spawn_size_diff(
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

fn join_size_diff(
  handle: thread::JoinHandle<Result<(Size, Size)>>,
) -> Result<(Size, Size)> {
  handle.join().map_err(|_| {
    tracing::error!("closure size thread panicked");
    eyre!("failed to get closure size due to thread error")
  })?
}

fn write_size_diff(
  writer: &mut impl fmt::Write,
  size_old: Size,
  size_new: Size,
) -> fmt::Result {
  let size_diff = size_new - size_old;

  writeln!(
    writer,
    "{header}: {size_old} -> {size_new}",
    header = "SIZE".bold(),
    size_old = size_old.red(),
    size_new = size_new.green(),
  )?;

  writeln!(
    writer,
    "{header}: {size_diff}",
    header = "DIFF".bold(),
    size_diff = if size_diff.bytes() > 0 {
      size_diff.green()
    } else {
      size_diff.red()
    },
  )
}
