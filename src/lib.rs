use std::path::{
  Path,
  PathBuf,
};

use misstep::{
  Report,
  Result,
  bail,
  report,
};

mod diff;
pub mod json;

pub use diff::{
  DiffStatus,
  Version,
  VersionAmount,
  VersionChange,
  VersionDiff,
};
mod render;
pub use render::{
  RenderOptions,
  write_diff_report,
  write_hidden_note,
};
mod report;
pub use report::{
  ChangeKind,
  DerivationSelectionStatus,
  DiffReport,
  PackageDiff,
  PackageSizeDelta,
  PathStats,
  diff_store_snapshots,
};

mod snapshot;
pub use snapshot::{
  SnapshotDocument,
  StoreSnapshot,
  query_snapshot_document,
};
mod store;
pub use store::StorePathInfo;

/// A validated store path. Always starts with `/nix/store`.
///
/// Can be created using `StorePath::try_from(path_buf)`.
#[derive(
  Debug,
  Clone,
  PartialEq,
  Eq,
  Hash,
  PartialOrd,
  Ord,
  serde::Serialize,
  serde::Deserialize,
)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct StorePath(String);

/// The directory every store path is directly inside of.
const STORE_DIR: &str = "/nix/store/";

/// Length of the hash at the start of a store path's file name.
const HASH_LEN: usize = 32;

impl From<StorePath> for PathBuf {
  fn from(path: StorePath) -> Self {
    path.0.into()
  }
}

impl std::ops::Deref for StorePath {
  type Target = Path;

  fn deref(&self) -> &Self::Target {
    Path::new(&self.0)
  }
}

impl TryFrom<PathBuf> for StorePath {
  type Error = Report;

  /// Accepts paths of the form `/nix/store/<hash>-<name>`.
  fn try_from(path: PathBuf) -> Result<Self> {
    tracing::trace!(path = %path.display(), "validating store path");
    let path = path.into_os_string().into_string().map_err(|path| {
      report!("path '{}' is not valid unicode", path.display())
    })?;

    let is_store_path = path
      .strip_prefix(STORE_DIR)
      .filter(|file_name| !file_name.contains('/'))
      .and_then(|file_name| file_name.split_once('-'))
      .is_some_and(|(hash, name)| {
        hash.len() == HASH_LEN
          && hash.bytes().all(|byte| byte.is_ascii_alphanumeric())
          && !name.is_empty()
      });
    if !is_store_path {
      bail!("path '{path}' is not of the form {STORE_DIR}<hash>-<name>");
    }

    Ok(Self(path))
  }
}

impl StorePath {
  /// Returns the path as a string.
  #[must_use]
  pub fn as_str(&self) -> &str {
    &self.0
  }

  /// Extracts the package name and possibly its version from the part of the
  /// file name after the hash.
  ///
  /// A digit-starting or Git-hash-like suffix is treated as the version.
  fn name_and_version(&self) -> (&str, Option<&str>) {
    // `try_from` guarantees `/nix/store/<hash>-<name>`.
    let name = &self.0[STORE_DIR.len() + HASH_LEN + 1..];
    split_name_and_version(strip_derivation_suffix(name))
  }
}

fn split_name_and_version(name: &str) -> (&str, Option<&str>) {
  for (index, _) in name.match_indices('-') {
    if index == 0 {
      continue;
    }

    let suffix = &name[index + 1..];
    if is_version_suffix(suffix) {
      return (&name[..index], Some(suffix));
    }
  }

  (name, None)
}

fn strip_derivation_suffix(name: &str) -> &str {
  name.strip_suffix(".drv").unwrap_or(name)
}

fn is_version_suffix(suffix: &str) -> bool {
  suffix
    .bytes()
    .next()
    .is_some_and(|byte| byte.is_ascii_digit())
    || diff::is_git_hash(suffix)
}

#[cfg(test)]
mod tests {
  use proptest::proptest;

  proptest! {
    #[test]
    fn parses_valid_paths(s in r"/nix/store/[a-z0-9A-Z]{32}-[-A-Za-z0-9._+~]{1,64}") {
      let path = PathBuf::from(s);
      let store_path = StorePath::try_from(path)
        .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
      let (_name, _version) = store_path.name_and_version();
    }
  }

  use super::*;

  #[test]
  fn test_store_path_from_nix_store() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path.clone())
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    assert_eq!(PathBuf::from(store_path), path);
  }

  #[test]
  fn test_invalid_store_path() {
    let path =
      PathBuf::from("/invalid/prefix/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path);
    assert!(store_path.is_err());
  }

  #[test]
  fn test_name_and_version_parsing_store_path() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();
    assert_eq!(name, "foo");
    assert_eq!(version, Some("1.0"));
  }

  #[test]
  fn test_name_and_version_parsing_hyphenated_version() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0-bin");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();
    assert_eq!(name, "foo");
    assert_eq!(version, Some("1.0-bin"));
  }

  #[test]
  fn test_name_and_version_parsing_git_hash_version() {
    let path = PathBuf::from(
      "/nix/store/0123456789abcdefghijklmnopqrstuv-helix-tree-sitter-pod-\
       cd1931314beafeebc957964c65802961e283411e",
    );
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();
    assert_eq!(name, "helix-tree-sitter-pod");
    assert_eq!(version, Some("cd1931314beafeebc957964c65802961e283411e"));
  }

  #[test]
  fn test_name_and_version_parsing_derivation_path() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0.drv");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();

    assert_eq!(name, "foo");
    assert_eq!(version, Some("1.0"));
  }

  #[test]
  fn test_name_and_version_parsing_derivation_patch() {
    let path = PathBuf::from(
      "/nix/store/0123456789abcdefghijklmnopqrstuv-CVE-2026-3184.patch.drv",
    );
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();

    assert_eq!(name, "CVE");
    assert_eq!(version, Some("2026-3184.patch"));
  }

  #[test]
  fn test_name_and_version_parsing_derivation_without_version() {
    let path = PathBuf::from(
      "/nix/store/0123456789abcdefghijklmnopqrstuv-unit-modprobe-.service.drv",
    );
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path.name_and_version();

    assert_eq!(name, "unit-modprobe-.service");
    assert_eq!(version, None);
  }

  #[test]
  fn test_store_path_rejects_malformed_paths() {
    for path in [
      "/nix/store/-0123456789abcdefghijklmnopqrstuv-foo-1.0",
      "/nix/store/0123456789abcdefghijklmnopqrstuv-",
      "/nix/store/0123456789abcdefghijklmnopqrstuv-foo/bin/foo",
      "/nix/store",
    ] {
      assert!(StorePath::try_from(PathBuf::from(path)).is_err(), "{path}");
    }
  }

  #[test]
  fn test_name_and_version_parsing_no_version() {
    let path = PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo");
    let store_path = StorePath::try_from(path).unwrap();
    let (name, version) = store_path.name_and_version();
    assert_eq!(name, "foo");
    assert_eq!(version, None);
  }

  #[test]
  fn test_unusual_store_paths() {
    let paths = vec![
      "/nix/store/0iav54v2brnmi2fv6bssla9k44z62cz7-po",
      "/nix/store/0i5i9mj0n4nry46qvzlmi6h1k9d3pbcn-gtk2-theme-paths.patch",
      "/nix/store/0dslh0d5kbgh40208jlf03n0zkjyc7cl-pkg-config-wrapper-0.29.\
       2-man",
      "/nix/store/0df8rz15sp4ai6md99q5qy9lf0srji5z-0001-Revert-libtool.\
       m4-fix-nm-BSD-flag-detection.patch",
      "/nix/store/0a1bxszp3c9rzphx8b6f5cb9ngbln6xj-unit-nix-daemon-.service",
    ];
    for p in paths {
      let store_path = StorePath::try_from(PathBuf::from(p)).unwrap();
      let (_name, _version) = store_path.name_and_version();
    }
  }
}
