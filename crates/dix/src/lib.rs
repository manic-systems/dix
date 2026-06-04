use std::path::{
  Path,
  PathBuf,
};

use derive_more::Deref;
use eyre::{
  Context as _,
  ContextCompat as _,
  Error,
  Result,
  bail,
  eyre,
};

#[cfg(feature = "json")] pub mod json;

pub use dix_diff::{
  DiffStatus,
  Version,
  VersionAmount,
  VersionDiff,
};
mod render;
pub use render::write_diff_report;
pub mod report;
pub use report::{
  DerivationSelectionStatus,
  DiffReport,
  PackageDiff,
  PackageSizeDelta,
  PathStats,
  query_diff_report,
};

pub mod store;

/// A validated store path. Always starts with `/nix/store`.
///
/// Can be created using `StorePath::try_from(path_buf)`.
#[derive(Deref, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorePath(PathBuf);

impl TryFrom<PathBuf> for StorePath {
  type Error = Error;

  fn try_from(path: PathBuf) -> Result<Self> {
    tracing::trace!(path = %path.display(), "validating store path");
    if !(path.starts_with("/nix/store") || path.starts_with("/tmp/")) {
      tracing::warn!(path = %path.display(), "path does not start with /nix/store or /tmp/");
      bail!(
        "path {path} must start with /nix/store or /tmp/",
        path = path.display(),
      );
    }
    tracing::trace!(path = %path.display(), "store path validated");
    Ok(Self(path))
  }
}

impl StorePath {
  /// Parses a Nix store path to extract the packages name and possibly its
  /// version.
  ///
  /// This function first extracts the store path name after the store hash. It
  /// then treats a digit-starting or Git-hash-like suffix as the version.
  fn parse_name_and_version(&self) -> Result<(&str, Option<&str>)> {
    let path = self.to_str().with_context(|| {
      format!(
        "failed to convert path '{path}' to valid unicode",
        path = self.display(),
      )
    })?;

    let file_name = self
      .file_name()
      .and_then(|file_name| file_name.to_str())
      .with_context(|| {
      format!("failed to extract valid unicode file name from path '{path}'")
    })?;

    let (store_hash, name) = file_name.split_once('-').ok_or_else(|| {
      eyre!("path '{path}' does not match expected Nix store format")
    })?;

    if store_hash.len() != 32
      || !store_hash.bytes().all(|byte| byte.is_ascii_alphanumeric())
      || name.is_empty()
    {
      bail!("path '{path}' does not match expected Nix store format");
    }

    let (name, version) = split_name_and_version(name);

    tracing::trace!(name = name, version = ?version, "parsed name and version from path");

    Ok((name, version))
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

fn is_version_suffix(suffix: &str) -> bool {
  suffix
    .bytes()
    .next()
    .is_some_and(|byte| byte.is_ascii_digit())
    || looks_like_git_hash_component(suffix)
}

fn looks_like_git_hash_component(component: &str) -> bool {
  (7..=40).contains(&component.len())
    && component.bytes().all(|byte| byte.is_ascii_hexdigit())
    && component
      .bytes()
      .any(|byte| matches!(byte, b'a'..=b'f' | b'A'..=b'F'))
}

fn path_to_canonical_string(path: &Path) -> Result<String> {
  let path = path.canonicalize().with_context(|| {
    format!(
      "failed to canonicalize path '{path}'",
      path = path.display(),
    )
  })?;

  let path = path.into_os_string().into_string().map_err(|path| {
    tracing::debug!("path contains invalid unicode characters");
    eyre!(
      "failed to convert path '{path}' to valid unicode",
      path = Path::new(&*path).display(), /* TODO: use .display() directly
                                           * after Rust 1.87.0 in flake. */
    )
  })?;

  Ok(path)
}

#[cfg(test)]
mod tests {
  use std::{
    fs,
    sync::OnceLock,
  };

  use proptest::proptest;
  use tempfile::TempDir;

  proptest! {
    #[test]
    fn parses_valid_paths(s in r"((/nix/store/)|(/tmp/[A-Za-z0-9._+-]+/))[a-z0-9A-Z]{32}-[-A-Za-z0-9._+~]{1,64}") {
      let path = PathBuf::from(s);
      let store_path = StorePath::try_from(path)
        .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
      let (_name, _version) = store_path.parse_name_and_version()
        .unwrap_or_else(|err| panic!("failed to get name and version: {err}"));
    }
  }

  use super::*;

  #[test]
  fn test_store_path_from_nix_store() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path.clone())
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let inner = store_path.0;
    assert_eq!(inner, path);
  }

  #[test]
  fn test_store_path_from_tmp_file() {
    let path =
      PathBuf::from("/tmp/test123/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path.clone())
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let inner = store_path.0;
    assert_eq!(inner, path);
  }

  #[test]
  fn test_invalid_store_path() {
    let path =
      PathBuf::from("/invalid/prefix/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path);
    assert!(store_path.is_err());
  }

  #[test]
  fn test_name_and_version_parsing_tmpfile() {
    let path =
      PathBuf::from("/tmp/test123/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path
      .parse_name_and_version()
      .unwrap_or_else(|err| panic!("failed to parse name and version: {err}"));
    assert_eq!(name, "foo");
    assert_eq!(version, Some("1.0"));
  }
  #[test]
  fn test_name_and_version_parsing_store_path() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path
      .parse_name_and_version()
      .unwrap_or_else(|err| panic!("failed to parse name and version: {err}"));
    assert_eq!(name, "foo");
    assert_eq!(version, Some("1.0"));
  }

  #[test]
  fn test_name_and_version_parsing_hyphenated_version() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0-bin");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let (name, version) = store_path
      .parse_name_and_version()
      .unwrap_or_else(|err| panic!("failed to parse name and version: {err}"));
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
    let (name, version) = store_path
      .parse_name_and_version()
      .unwrap_or_else(|err| panic!("failed to parse name and version: {err}"));
    assert_eq!(name, "helix-tree-sitter-pod");
    assert_eq!(version, Some("cd1931314beafeebc957964c65802961e283411e"));
  }

  #[test]
  fn test_name_and_version_parsing_invalid_prefix() {
    let path =
      PathBuf::from("/nix/store/-0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path = StorePath::try_from(path)
      .unwrap_or_else(|err| panic!("failed to create StorePath: {err}"));
    let parsed = store_path.parse_name_and_version();
    assert!(parsed.is_err());
  }

  #[test]
  fn test_name_and_version_parsing_no_version() {
    let path = PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo");
    let store_path = StorePath::try_from(path).unwrap();
    let (name, version) = store_path.parse_name_and_version().unwrap();
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
      let (_name, _version) = store_path.parse_name_and_version().unwrap();
    }
  }

  /// returns a temporary directory path (or creates it)
  fn get_temp_dir() -> &'static Path {
    static TEMP_DIR: OnceLock<TempDir> = OnceLock::new();
    TEMP_DIR
      .get_or_init(|| {
        TempDir::new().unwrap_or_else(|err| {
          panic!("failed to create temporary directory: {err}")
        })
      })
      .path()
  }

  #[test]
  fn test_path_to_canonical_string_basic() {
    let dir = get_temp_dir();
    let path = dir.join("simple-basic-path");
    fs::write(&path, "").unwrap();
    let canonical = path_to_canonical_string(&path).unwrap();
    assert_eq!(canonical, path);
  }

  #[test]
  #[cfg(unix)]
  fn test_path_to_canonical_string_with_symlink() {
    let dir = get_temp_dir();
    let path = dir.join("symlink-path");
    let target = dir.join("target-path");
    fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let canonical = path_to_canonical_string(&path).unwrap();
    assert_eq!(canonical, target);
  }

  #[test]
  #[cfg(unix)]
  fn test_path_to_canonical_string_invalid_unicode() {
    use std::{
      ffi::OsString,
      os::unix::ffi::OsStringExt,
    };

    let dir = get_temp_dir();
    let path = dir.join(OsString::from_vec(
      b"invalid-unicode"
        .iter()
        .chain(&[0xFFu8, 0xFE])
        .copied()
        .collect(),
    ));
    std::fs::write(&path, "").unwrap();
    let canonical = path_to_canonical_string(&path);
    assert!(canonical.is_err());
  }
}
