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
    add_selection_status,
    collect_path_versions,
    collect_system_names,
  },
  generate_diffs_from_paths,
  store::{
    CombinedStoreBackend,
    StoreBackend,
  },
};

pub fn display_diff(
  path_old: &PathBuf,
  path_new: &PathBuf,
  force_correctness: bool,
) -> Result<()> {
  let mut connection = CombinedStoreBackend::for_correctness(force_correctness);
  connection.connect()?;
  generate_diff(&mut std::io::stdout(), path_old, path_new, &connection)
}

fn generate_diff(
  out: &mut dyn Write,
  path_old: &PathBuf,
  path_new: &PathBuf,
  backend: &impl StoreBackend,
) -> Result<()> {
  // Query dependencies for old path
  let paths_old = backend.query_dependents(path_old).with_context(|| {
    format!("failed to query dependencies of '{}'", path_old.display())
  })?;

  // Query dependencies for new path
  let paths_new = backend.query_dependents(path_new).with_context(|| {
    format!("failed to query dependencies of '{}'", path_new.display())
  })?;

  // Query system derivations for old path
  let system_derivations_old = backend
    .query_system_derivations(path_old)
    .with_context(|| {
      format!(
        "failed to query system derivations of '{}'",
        path_old.display()
      )
    })?;

  // Query system derivations for new path
  let system_derivations_new = backend
    .query_system_derivations(path_new)
    .with_context(|| {
      format!(
        "failed to query system derivations of '{}'",
        path_new.display()
      )
    })?;

  let paths_map = collect_path_versions(paths_old, paths_new);
  let sys_old_set = collect_system_names(system_derivations_old, "old");
  let sys_new_set = collect_system_names(system_derivations_new, "new");

  let mut diffs = generate_diffs_from_paths(paths_map);
  // Make sure the diffs are always in the same order so
  // our tests testing against the output don't fail nondeterministically.
  for diff in &mut diffs {
    diff.new.sort();
    diff.old.sort();
  }
  diffs.sort();
  add_selection_status(&mut diffs, &sys_old_set, &sys_new_set);
  let size_old = backend.query_closure_size(path_old)?.bytes();
  let size_new = backend.query_closure_size(path_new)?.bytes();

  serde_json::to_writer(out, &JsonReport {
    diffs,
    size_old,
    size_new,
  })
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

#[cfg(test)]
mod tests {

  use super::*;
  use crate::store::{
    DbConnection,
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
    let mut db = DbConnection::new(&db_path);
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
