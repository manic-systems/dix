use std::{
  io::Write,
  path::Path,
};

use dix_diff::{
  Diff,
  DiffReport,
};
use eyre::{
  Result,
  WrapErr as _,
};
use serde::Serialize;

use crate::query_diff_report;

/// Writes the diff report as JSON.
///
/// # Errors
///
/// Returns an error if querying the diff report or writing JSON fails.
pub fn display_diff(
  path_old: &Path,
  path_new: &Path,
  force_correctness: bool,
) -> Result<()> {
  let report = query_diff_report(path_old, path_new, force_correctness)?;
  generate_diff(&mut std::io::stdout(), &report)
}

fn generate_diff(out: &mut dyn Write, report: &DiffReport) -> Result<()> {
  serde_json::to_writer(out, &JsonReport::from(report))
    .context("Failed to write json output.")
}

#[derive(Serialize)]
pub struct JsonReport<'a> {
  /// package changes
  diffs:    &'a [Diff],
  /// old closure size (in bytes)
  size_old: i64,
  /// new closure size (in bytes)
  size_new: i64,
}

impl<'a> From<&'a DiffReport> for JsonReport<'a> {
  fn from(report: &'a DiffReport) -> Self {
    Self {
      diffs:    report.diffs.as_slice(),
      size_old: report.size_old.bytes(),
      size_new: report.size_new.bytes(),
    }
  }
}

#[cfg(test)]
mod tests {

  use dix_diff::{
    Change,
    DerivationSelectionStatus,
    Diff,
    DiffReport,
    DiffStatus,
    Version,
  };
  use size::Size;

  use super::*;

  #[test]
  fn test_basic_json_output_format() {
    let expected_output = r#"{"diffs":[{"name":"nixos","old":[{"name":"25.11-system-path","amount":1},{"name":"25.11-system","amount":1}],"new":[{"name":"25.12-system-path","amount":1},{"name":"25.12-system","amount":1}],"status":{"Changed":"Upgraded"},"selection":"Unselected","has_common_versions":false}],"size_old":115001000,"size_new":115001000}"#;

    let report = DiffReport {
      diffs:    vec![Diff {
        name:                "nixos".to_owned(),
        old:                 vec![
          Version::new("25.11-system-path"),
          Version::new("25.11-system"),
        ],
        new:                 vec![
          Version::new("25.12-system-path"),
          Version::new("25.12-system"),
        ],
        status:              DiffStatus::Changed(Change::Upgraded),
        selection:           DerivationSelectionStatus::Unselected,
        has_common_versions: false,
      }],
      size_old: Size::from_bytes(115_001_000),
      size_new: Size::from_bytes(115_001_000),
    };

    let mut actual_output = Vec::new();
    generate_diff(&mut actual_output, &report).unwrap();
    let actual_output = String::from_utf8(actual_output).unwrap();
    assert_eq!(expected_output, &actual_output);
  }
}
