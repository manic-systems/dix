use size::Size;

use crate::{
  Diff,
  engine,
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
