mod engine;
mod matching;
mod model;
mod snapshot;
mod version;

pub use engine::diff_snapshots;
pub use model::{
  Diff,
  DiffStatus,
  VersionAmount,
  VersionDiff,
};
pub use snapshot::{
  Package,
  PackageSnapshot,
};
pub use version::{
  Version,
  VersionComponent,
  VersionPiece,
};
