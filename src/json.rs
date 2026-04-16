use std::{
  io::Write,
  path::PathBuf,
};

use eyre::{
  Result,
  WrapErr as _,
};
use serde::Serialize;

use crate::{
  diff::{
    Diff,
    package_diff_report,
  },
  snapshot::{
    SnapshotDiffReport,
    diff_snapshots,
    gather_snapshot,
  },
  store::StoreBackend,
};

pub fn display_diff(
  path_old: &PathBuf,
  path_new: &PathBuf,
  force_correctness: bool,
) -> Result<()> {
  let report = package_diff_report(path_old, path_new, force_correctness)?;
  write_json_report(&mut std::io::stdout(), report)
}

fn generate_diff<'a>(
  out: &mut dyn Write,
  path_old: &PathBuf,
  path_new: &PathBuf,
  backend: &impl StoreBackend<'a>,
) -> Result<()> {
  let old = gather_snapshot(path_old, backend)
    .with_context(|| format!("failed to gather snapshot for '{}'", path_old.display()))?;
  let new = gather_snapshot(path_new, backend)
    .with_context(|| format!("failed to gather snapshot for '{}'", path_new.display()))?;
  let report = diff_snapshots(&old, &new)?;
  write_json_report(out, report)
}

fn write_json_report(
  out: &mut dyn Write,
  report: SnapshotDiffReport,
) -> Result<()> {
  serde_json::to_writer(out, &JsonReport::from(report))
    .context("Failed to write json output.")
}

#[derive(Serialize)]
pub struct JsonReport {
  /// package changes
  diffs:    Vec<Diff>,
  /// old closure size (in bytes)
  size_old: i64,
  /// new closure size (in bytes)
  size_new: i64,
}

impl From<SnapshotDiffReport> for JsonReport {
  fn from(report: SnapshotDiffReport) -> Self {
    Self {
      diffs: report.diffs,
      size_old: report.size_old,
      size_new: report.size_new,
    }
  }
}

#[cfg(test)]
mod tests {

  use super::*;
  use crate::store::{
    LazyDBConnection,
    test_utils::{
      self,
      // TestDbBuilder,
      fixtures,
    },
  };
  #[test]
  fn test_basic_json_output_format() {
    let db_builder =
      test_utils::create_system_test_db().expect("failed to create test db");
    let db_path = db_builder.db_path().to_string_lossy().to_string();
    let mut db = LazyDBConnection::new(&db_path);
    db.connect().unwrap();
    let system_old =
      db_builder.resolve_fixture_path(&fixtures::system_path("nixos-25.11"));
    let system_new =
      db_builder.resolve_fixture_path(&fixtures::system_path("nixos-25.12"));

    let expected_output = r#"{"diffs":[{"name":"nixos","old":[{"name":"25.11-system-path","amount":1},{"name":"25.11-system","amount":1}],"new":[{"name":"25.12-system-path","amount":1},{"name":"25.12-system","amount":1}],"status":{"Changed":"Upgraded"},"selection":"Unselected","has_common_versions":false}],"size_old":115001000,"size_new":115001000}"#;

    let mut actual_output = Vec::new();
    generate_diff(
      &mut actual_output,
      &PathBuf::from(system_old),
      &PathBuf::from(system_new),
      &db,
    )
    .unwrap();
    let actual_output = String::from_utf8(actual_output).unwrap();
    assert_eq!(expected_output, &actual_output);
  }
}
