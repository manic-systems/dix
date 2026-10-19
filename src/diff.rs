//! The pure package/version diff engine.
//!
//! Nothing in here knows about the Nix store; the store-backed report in
//! [`crate::report`] feeds it parsed packages.
mod engine;
mod matching;
mod model;
mod version;

pub use engine::diff_packages;
pub use model::{
  DiffStatus,
  Package,
  VersionAmount,
  VersionChange,
  VersionDiff,
};
pub use version::{
  Version,
  VersionPiece,
  is_git_hash,
};
