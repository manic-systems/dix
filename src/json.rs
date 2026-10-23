use serde::Serialize;

use crate::{
  ChangeKind,
  DerivationSelectionStatus,
  DiffReport,
  DiffStatus,
  PackageDiff,
  PathStats,
  Version,
  VersionAmount,
  VersionDiff,
};

#[derive(Serialize)]
pub struct JsonReport<'a> {
  /// package changes
  diffs:    Vec<JsonDiff<'a>>,
  /// exact closure path counts
  paths:    JsonPathStats,
  /// old closure size (in bytes)
  size_old: i64,
  /// new closure size (in bytes)
  size_new: i64,
}

impl<'a> From<&'a DiffReport> for JsonReport<'a> {
  fn from(report: &'a DiffReport) -> Self {
    Self {
      diffs:    report.diffs.iter().map(JsonDiff::from).collect(),
      paths:    JsonPathStats::from(report.path_stats),
      size_old: report.size_old.bytes(),
      size_new: report.size_new.bytes(),
    }
  }
}

#[derive(Serialize)]
struct JsonPathStats {
  old:     usize,
  new:     usize,
  added:   usize,
  removed: usize,
}

impl From<PathStats> for JsonPathStats {
  fn from(stats: PathStats) -> Self {
    Self {
      old:     stats.old,
      new:     stats.new,
      added:   stats.added,
      removed: stats.removed,
    }
  }
}

#[derive(Serialize)]
struct JsonDiff<'a> {
  name:                 &'a str,
  versions:             Vec<JsonVersionDiff<'a>>,
  status:               DiffStatus,
  kind:                 ChangeKind,
  selection:            DerivationSelectionStatus,
  has_omitted_versions: bool,
  size_old:             i64,
  size_new:             i64,
  size_delta:           i64,
}

impl<'a> From<&'a PackageDiff> for JsonDiff<'a> {
  fn from(diff: &'a PackageDiff) -> Self {
    let size_delta = diff.size.delta();
    Self {
      name:                 diff.name.as_str(),
      versions:             diff
        .change
        .iter()
        .flat_map(|change| &change.versions)
        .map(JsonVersionDiff::from)
        .collect(),
      status:               diff.status(),
      kind:                 diff.kind(),
      selection:            diff.selection,
      has_omitted_versions: diff
        .change
        .as_ref()
        .is_some_and(|change| change.has_omitted_versions),
      size_old:             diff.size.old.bytes(),
      size_new:             diff.size.new.bytes(),
      size_delta:           size_delta.bytes(),
    }
  }
}

#[derive(Serialize)]
struct JsonVersion<'a> {
  /// `null` for packages without a version.
  name: Option<&'a str>,
}

impl<'a> From<Option<&'a Version>> for JsonVersion<'a> {
  fn from(version: Option<&'a Version>) -> Self {
    Self {
      name: version.map(|version| version.name.as_str()),
    }
  }
}

#[derive(Serialize)]
struct JsonVersionAmount<'a> {
  /// `null` for packages without a version.
  name:   Option<&'a str>,
  amount: usize,
}

impl<'a> From<&'a VersionAmount> for JsonVersionAmount<'a> {
  fn from(version: &'a VersionAmount) -> Self {
    Self {
      name:   version
        .version
        .as_ref()
        .map(|version| version.name.as_str()),
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
          version:    JsonVersion::from(version.as_ref()),
          old_amount: old_amount.get(),
          new_amount: new_amount.get(),
        }
      },
    }
  }
}

#[cfg(test)]
mod tests {
  use std::num::NonZeroUsize;

  use size::Size;

  use super::*;
  use crate::{
    DerivationSelectionStatus,
    DiffReport,
    PackageDiff,
    PackageSizeDelta,
    diff::VersionChange,
  };

  fn amount(amount: usize) -> NonZeroUsize {
    NonZeroUsize::new(amount)
      .unwrap_or_else(|| panic!("test version amount must be nonzero"))
  }

  #[test]
  fn test_basic_json_output_format() {
    let expected_output = r#"{"diffs":[{"name":"nixos","versions":[{"kind":"changed","old":{"name":"25.11-system-path","amount":1},"new":{"name":"25.12-system-path","amount":1}},{"kind":"amount_changed","version":{"name":"25.12-system"},"old_amount":1,"new_amount":2}],"status":"Changed","kind":"version","selection":"Unselected","has_omitted_versions":false,"size_old":1000,"size_new":2500,"size_delta":1500}],"paths":{"old":7529,"new":7536,"added":5054,"removed":5047},"size_old":115001000,"size_new":115001000}"#;

    let report = DiffReport {
      diffs:      vec![PackageDiff {
        name:      "nixos".to_owned(),
        change:    Some(VersionChange {
          versions:             vec![
            VersionDiff::Changed {
              old: VersionAmount::new("25.11-system-path", amount(1)),
              new: VersionAmount::new("25.12-system-path", amount(1)),
            },
            VersionDiff::AmountChanged {
              version:    Some(Version::new("25.12-system")),
              old_amount: amount(1),
              new_amount: amount(2),
            },
          ],
          status:               DiffStatus::Changed,
          has_omitted_versions: false,
        }),
        selection: DerivationSelectionStatus::Unselected,
        size:      PackageSizeDelta {
          old: Size::from_bytes(1_000),
          new: Size::from_bytes(2_500),
        },
      }],
      path_stats: PathStats {
        old:     7529,
        new:     7536,
        added:   5054,
        removed: 5047,
      },
      size_old:   Size::from_bytes(115_001_000),
      size_new:   Size::from_bytes(115_001_000),
    };

    let actual_output =
      serde_json::to_string(&JsonReport::from(&report)).unwrap();
    assert_eq!(expected_output, &actual_output);
  }
}
