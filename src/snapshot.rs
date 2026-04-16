use std::{
  collections::HashSet,
  path::Path,
};

use eyre::{Result, WrapErr as _, bail};

use crate::{
  StorePath,
  diff::{
    Diff, add_selection_status, collect_path_versions, collect_system_names,
  },
  generate_diffs_from_paths,
  store::StoreBackend,
};

/// A detached, normalized closure snapshot.
///
/// This value is intentionally independent from any live store backend so
/// callers can gather closure facts remotely and later diff them locally.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct ClosureSnapshot {
  /// Root path the snapshot was gathered from.
  pub root_path: StorePath,
  /// Total transitive closure size in bytes.
  pub closure_size_bytes: i64,
  /// All store paths in the transitive closure of the root.
  pub closure_paths: Vec<StorePath>,
  /// Directly selected system package paths from `<root>/sw`.
  pub selected_paths: Vec<StorePath>,
}

/// A package diff report produced from two detached closure snapshots.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct SnapshotDiffReport {
  /// package changes
  pub diffs: Vec<Diff>,
  /// old closure size (in bytes)
  pub size_old: i64,
  /// new closure size (in bytes)
  pub size_new: i64,
}

/// Computes a package diff report from two detached closure snapshots.
///
/// The function is pure over already-normalized snapshot data. It does not
/// query a live Nix store and does not require the compared roots to exist on
/// the local machine.
pub fn diff_snapshots(
  old: &ClosureSnapshot,
  new: &ClosureSnapshot,
) -> Result<SnapshotDiffReport> {
  validate_snapshot(old)?;
  validate_snapshot(new)?;

  let paths_map = collect_path_versions(
    old.closure_paths.iter().cloned(),
    new.closure_paths.iter().cloned(),
  );
  let sys_old_set =
    collect_system_names(old.selected_paths.iter().cloned(), "old snapshot");
  let sys_new_set =
    collect_system_names(new.selected_paths.iter().cloned(), "new snapshot");

  let mut diffs = generate_diffs_from_paths(paths_map);
  for diff in &mut diffs {
    diff.new.sort();
    diff.old.sort();
  }
  add_selection_status(&mut diffs, &sys_old_set, &sys_new_set);
  diffs
    .sort_by(|a, b| a.status.cmp(&b.status).then_with(|| a.name.cmp(&b.name)));

  Ok(SnapshotDiffReport {
    diffs,
    size_old: old.closure_size_bytes,
    size_new: new.closure_size_bytes,
  })
}

pub(crate) fn gather_snapshot<'a>(
  path: &Path,
  backend: &impl StoreBackend<'a>,
) -> Result<ClosureSnapshot> {
  tracing::debug!(path = %path.display(), "gathering closure snapshot");
  let root_path = match StorePath::try_from(path.to_path_buf()) {
    Ok(path) => path,
    Err(_) => {
      let canonical = path.canonicalize().wrap_err_with(|| {
        format!("failed to canonicalize path '{}'", path.display())
      })?;
      StorePath::try_from(canonical)?
    },
  };

  tracing::trace!(path = %path.display(), "querying closure size for snapshot");
  let closure_size_bytes = backend.query_closure_size(path)?.bytes();
  tracing::trace!(path = %path.display(), "querying closure paths for snapshot");
  let closure_paths = backend.query_dependents(path)?.collect::<Vec<_>>();
  tracing::trace!(path = %path.display(), "querying selected system paths for snapshot");
  let selected_paths = backend.query_system_derivations(path)?.collect::<Vec<_>>();

  let snapshot = ClosureSnapshot {
    root_path,
    closure_size_bytes,
    closure_paths,
    selected_paths,
  };
  validate_snapshot(&snapshot)?;
  tracing::debug!(
    root_path = %snapshot.root_path.display(),
    closure_paths = snapshot.closure_paths.len(),
    selected_paths = snapshot.selected_paths.len(),
    closure_size_bytes = snapshot.closure_size_bytes,
    "gathered closure snapshot"
  );
  Ok(snapshot)
}

fn validate_snapshot(snapshot: &ClosureSnapshot) -> Result<()> {
  let closure_paths = snapshot.closure_paths.iter().collect::<HashSet<_>>();

  for selected_path in &snapshot.selected_paths {
    if !closure_paths.contains(selected_path) {
      bail!(
        "selected path '{}' is not present in closure snapshot for '{}'",
        selected_path.display(),
        snapshot.root_path.display()
      );
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use crate::{
    diff::{Change, DerivationSelectionStatus, DiffStatus},
    store::{
      LazyDBConnection, StoreBackend,
      test_utils::{self, fixtures},
    },
  };

  use super::*;

  fn store_path(hash: &str, suffix: &str) -> StorePath {
    StorePath::try_from(PathBuf::from(format!(
      "/tmp/dix-snapshot-tests/{hash}-{suffix}"
    )))
    .unwrap()
  }

  #[test]
  fn diff_snapshots_reports_semantic_changes_from_detached_data() {
    let old = ClosureSnapshot {
      root_path: store_path(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "nixos-system-old",
      ),
      closure_size_bytes: 100,
      closure_paths: vec![
        store_path("11111111111111111111111111111111", "bash-5.2"),
        store_path("22222222222222222222222222222222", "nano-8.0"),
        store_path("33333333333333333333333333333333", "glibc-2.42"),
      ],
      selected_paths: vec![
        store_path("11111111111111111111111111111111", "bash-5.2"),
        store_path("22222222222222222222222222222222", "nano-8.0"),
      ],
    };

    let new = ClosureSnapshot {
      root_path: store_path(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "nixos-system-new",
      ),
      closure_size_bytes: 120,
      closure_paths: vec![
        store_path("44444444444444444444444444444444", "bash-5.3"),
        store_path("33333333333333333333333333333333", "glibc-2.42"),
        store_path("55555555555555555555555555555555", "ripgrep-14.1"),
      ],
      selected_paths: vec![
        store_path("44444444444444444444444444444444", "bash-5.3"),
        store_path("55555555555555555555555555555555", "ripgrep-14.1"),
      ],
    };

    let report = diff_snapshots(&old, &new).unwrap();

    assert_eq!(report.size_old, 100);
    assert_eq!(report.size_new, 120);
    assert_eq!(report.diffs.len(), 3);

    let bash = report
      .diffs
      .iter()
      .find(|diff| diff.name == "bash")
      .unwrap();
    assert_eq!(bash.status, DiffStatus::Changed(Change::Upgraded));
    assert_eq!(bash.selection, DerivationSelectionStatus::Selected);

    let ripgrep = report
      .diffs
      .iter()
      .find(|diff| diff.name == "ripgrep")
      .unwrap();
    assert_eq!(ripgrep.status, DiffStatus::Added);
    assert_eq!(ripgrep.selection, DerivationSelectionStatus::NewlySelected);

    let nano = report
      .diffs
      .iter()
      .find(|diff| diff.name == "nano")
      .unwrap();
    assert_eq!(nano.status, DiffStatus::Removed);
    assert_eq!(nano.selection, DerivationSelectionStatus::NewlyUnselected);
  }

  #[test]
  fn diff_snapshots_rejects_selected_paths_outside_the_closure() {
    let snapshot = ClosureSnapshot {
      root_path: store_path(
        "cccccccccccccccccccccccccccccccc",
        "nixos-system-invalid",
      ),
      closure_size_bytes: 42,
      closure_paths: vec![store_path(
        "66666666666666666666666666666666",
        "bash-5.3",
      )],
      selected_paths: vec![store_path(
        "77777777777777777777777777777777",
        "ripgrep-14.1",
      )],
    };

    let err = diff_snapshots(&snapshot, &snapshot).unwrap_err();
    assert!(
      err.to_string().contains("selected path")
        && err.to_string().contains("not present in closure snapshot")
    );
  }

  #[test]
  fn diff_snapshots_matches_fixture_queries_without_live_diff_queries() {
    let db_builder = test_utils::create_system_test_db().unwrap();
    let db_path = db_builder.db_path().to_string_lossy().to_string();
    let mut db = LazyDBConnection::new(&db_path);
    db.connect().unwrap();

    let system_old =
      db_builder.resolve_fixture_path(&fixtures::system_path("nixos-25.11"));
    let system_new =
      db_builder.resolve_fixture_path(&fixtures::system_path("nixos-25.12"));

    let old = ClosureSnapshot {
      root_path: StorePath::try_from(PathBuf::from(system_old.clone()))
        .unwrap(),
      closure_size_bytes: db.query_closure_size(&system_old).unwrap().bytes(),
      closure_paths: db.query_dependents(&system_old).unwrap().collect(),
      selected_paths: db
        .query_system_derivations(&system_old)
        .unwrap()
        .collect(),
    };

    let new = ClosureSnapshot {
      root_path: StorePath::try_from(PathBuf::from(system_new.clone()))
        .unwrap(),
      closure_size_bytes: db.query_closure_size(&system_new).unwrap().bytes(),
      closure_paths: db.query_dependents(&system_new).unwrap().collect(),
      selected_paths: db
        .query_system_derivations(&system_new)
        .unwrap()
        .collect(),
    };

    let report = diff_snapshots(&old, &new).unwrap();

    assert_eq!(report.size_old, 115001000);
    assert_eq!(report.size_new, 115001000);
    assert_eq!(report.diffs.len(), 1);
    assert_eq!(report.diffs[0].name, "nixos");
    assert_eq!(
      report.diffs[0].status,
      DiffStatus::Changed(Change::Upgraded)
    );
    assert_eq!(
      report.diffs[0].selection,
      DerivationSelectionStatus::Unselected
    );
    assert!(!report.diffs[0].has_common_versions);
  }
}
