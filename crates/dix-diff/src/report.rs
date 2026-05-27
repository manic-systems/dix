use std::fmt;

use size::Size;
use yansi::Paint as _;

use crate::{
  Diff,
  engine,
  render,
  snapshot::PackageSnapshot,
};

#[derive(Debug)]
pub struct DiffReport {
  pub diffs:    Vec<Diff>,
  pub size_old: Size,
  pub size_new: Size,
}

impl DiffReport {
  #[must_use]
  pub fn between(old: PackageSnapshot, new: PackageSnapshot) -> Self {
    let mut diffs = engine::diff_snapshots(&old, &new);
    engine::canonicalize_diffs(&mut diffs);

    Self {
      diffs,
      size_old: old.closure_size,
      size_new: new.closure_size,
    }
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

  let wrote = render::render_package_diffs(writer, &report.diffs)?;

  if wrote > 0 {
    writeln!(writer)?;
  }

  write_size_diff(writer, report.size_old, report.size_new)?;

  Ok(wrote)
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
