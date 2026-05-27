use std::{
  io::Write,
  path::Path,
};

use dix_diff::{
  DiffStatus,
  Version,
  VersionAmount,
  VersionDiff,
};
use eyre::{
  Result,
  WrapErr as _,
};
use serde::Serialize;

use crate::{
  DerivationSelectionStatus,
  DiffReport,
  PackageDiff,
  query_diff_report,
};

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
  diffs:    Vec<JsonDiff<'a>>,
  /// old closure size (in bytes)
  size_old: i64,
  /// new closure size (in bytes)
  size_new: i64,
}

impl<'a> From<&'a DiffReport> for JsonReport<'a> {
  fn from(report: &'a DiffReport) -> Self {
    Self {
      diffs:    report.diffs.iter().map(JsonDiff::from).collect(),
      size_old: report.size_old.bytes(),
      size_new: report.size_new.bytes(),
    }
  }
}

#[derive(Serialize)]
struct JsonDiff<'a> {
  name:                 &'a str,
  versions:             Vec<JsonVersionDiff<'a>>,
  status:               JsonDiffStatus,
  selection:            JsonDerivationSelectionStatus,
  has_omitted_versions: bool,
}

impl<'a> From<&'a PackageDiff> for JsonDiff<'a> {
  fn from(diff: &'a PackageDiff) -> Self {
    Self {
      name:                 diff.name.as_str(),
      versions:             diff
        .versions
        .iter()
        .map(JsonVersionDiff::from)
        .collect(),
      status:               JsonDiffStatus::from(diff.status),
      selection:            JsonDerivationSelectionStatus::from(diff.selection),
      has_omitted_versions: diff.has_omitted_versions,
    }
  }
}

#[derive(Serialize)]
struct JsonVersion<'a> {
  name: &'a str,
}

impl<'a> From<&'a Version> for JsonVersion<'a> {
  fn from(version: &'a Version) -> Self {
    Self {
      name: version.name.as_str(),
    }
  }
}

#[derive(Serialize)]
struct JsonVersionAmount<'a> {
  name:   &'a str,
  amount: usize,
}

impl<'a> From<&'a VersionAmount> for JsonVersionAmount<'a> {
  fn from(version: &'a VersionAmount) -> Self {
    Self {
      name:   version.version.name.as_str(),
      amount: version.amount.get(),
    }
  }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonVersionDiff<'a> {
  Removed {
    version: JsonVersionAmount<'a>,
  },
  Added {
    version: JsonVersionAmount<'a>,
  },
  Changed {
    old: JsonVersionAmount<'a>,
    new: JsonVersionAmount<'a>,
  },
  AmountChanged {
    version:    JsonVersion<'a>,
    old_amount: usize,
    new_amount: usize,
  },
}

impl<'a> From<&'a VersionDiff> for JsonVersionDiff<'a> {
  fn from(version_diff: &'a VersionDiff) -> Self {
    match version_diff {
      VersionDiff::Removed(version) => {
        Self::Removed {
          version: JsonVersionAmount::from(version),
        }
      },
      VersionDiff::Added(version) => {
        Self::Added {
          version: JsonVersionAmount::from(version),
        }
      },
      VersionDiff::Changed { old, new } => {
        Self::Changed {
          old: JsonVersionAmount::from(old),
          new: JsonVersionAmount::from(new),
        }
      },
      VersionDiff::AmountChanged {
        version,
        old_amount,
        new_amount,
      } => {
        Self::AmountChanged {
          version:    JsonVersion::from(version),
          old_amount: old_amount.get(),
          new_amount: new_amount.get(),
        }
      },
    }
  }
}

#[derive(Serialize)]
enum JsonDiffStatus {
  Changed,
  Mixed,
  Upgraded,
  Downgraded,
  Added,
  Removed,
}

impl From<DiffStatus> for JsonDiffStatus {
  fn from(status: DiffStatus) -> Self {
    match status {
      DiffStatus::Changed => Self::Changed,
      DiffStatus::Mixed => Self::Mixed,
      DiffStatus::Upgraded => Self::Upgraded,
      DiffStatus::Downgraded => Self::Downgraded,
      DiffStatus::Added => Self::Added,
      DiffStatus::Removed => Self::Removed,
    }
  }
}

#[derive(Serialize)]
enum JsonDerivationSelectionStatus {
  Selected,
  NewlySelected,
  Unselected,
  NewlyUnselected,
}

impl From<DerivationSelectionStatus> for JsonDerivationSelectionStatus {
  fn from(status: DerivationSelectionStatus) -> Self {
    match status {
      DerivationSelectionStatus::Selected => Self::Selected,
      DerivationSelectionStatus::NewlySelected => Self::NewlySelected,
      DerivationSelectionStatus::Unselected => Self::Unselected,
      DerivationSelectionStatus::NewlyUnselected => Self::NewlyUnselected,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::num::NonZeroUsize;

  use dix_diff::{
    DiffStatus,
    Version,
    VersionAmount,
    VersionDiff,
  };
  use size::Size;

  use super::*;
  use crate::{
    DerivationSelectionStatus,
    DiffReport,
    PackageDiff,
  };

  fn amount(amount: usize) -> NonZeroUsize {
    NonZeroUsize::new(amount)
      .unwrap_or_else(|| panic!("test version amount must be nonzero"))
  }

  #[test]
  fn test_basic_json_output_format() {
    let expected_output = r#"{"diffs":[{"name":"nixos","versions":[{"kind":"changed","old":{"name":"25.11-system-path","amount":1},"new":{"name":"25.12-system-path","amount":1}},{"kind":"amount_changed","version":{"name":"25.12-system"},"old_amount":1,"new_amount":2}],"status":"Changed","selection":"Unselected","has_omitted_versions":false}],"size_old":115001000,"size_new":115001000}"#;

    let report = DiffReport {
      diffs:    vec![PackageDiff {
        name:                 "nixos".to_owned(),
        versions:             vec![
          VersionDiff::Changed {
            old: VersionAmount::new("25.11-system-path", amount(1)),
            new: VersionAmount::new("25.12-system-path", amount(1)),
          },
          VersionDiff::AmountChanged {
            version:    Version::new("25.12-system"),
            old_amount: amount(1),
            new_amount: amount(2),
          },
        ],
        status:               DiffStatus::Changed,
        selection:            DerivationSelectionStatus::Unselected,
        has_omitted_versions: false,
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
