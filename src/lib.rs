use std::{
  ops::Deref,
  path::{
    Path,
    PathBuf,
  },
};

use dix_diff::Version;
use eyre::{
  Context as _,
  ContextCompat as _,
  Error,
  Result,
  bail,
  eyre,
};

#[cfg(feature = "json")] pub mod json;

pub use dix_diff::DiffReport;

pub mod report;
pub use report::query_diff_report;

mod render;
pub use render::write_diff_report;

pub mod store;

/// A validated Nix store path.
///
/// Accepts canonical `/nix/store` paths and temporary store paths produced
/// under `/tmp`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorePath {
  path:   PathBuf,
  parsed: ParsedStorePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ParsedStorePath {
  name:    String,
  version: Option<Version>,
}

impl TryFrom<PathBuf> for StorePath {
  type Error = Error;

  fn try_from(path: PathBuf) -> Result<Self> {
    tracing::trace!(path = %path.display(), "validating store path");
    let path_text = path.to_str().with_context(|| {
      format!(
        "failed to convert path '{path}' to valid unicode",
        path = path.display(),
      )
    })?;
    let parsed = parse_store_path(path_text)?;

    tracing::trace!(path = %path.display(), "store path validated");
    Ok(Self { path, parsed })
  }
}

impl Deref for StorePath {
  type Target = Path;

  fn deref(&self) -> &Self::Target {
    &self.path
  }
}

impl AsRef<Path> for StorePath {
  fn as_ref(&self) -> &Path {
    &self.path
  }
}

impl StorePath {
  pub(crate) fn package_name(&self) -> &str {
    &self.parsed.name
  }

  pub(crate) const fn package_version(&self) -> Option<&Version> {
    self.parsed.version.as_ref()
  }
}

fn parse_store_path(path: &str) -> Result<ParsedStorePath> {
  let store_name = store_name_from_path(path)?;
  let parsed = split_name_and_version(store_name)?;

  tracing::trace!(name = parsed.name.as_str(), version = ?parsed.version, "parsed name and version from path");

  Ok(parsed)
}

fn store_name_from_path(path: &str) -> Result<&str> {
  let store_name = if let Some(rest) = path.strip_prefix("/nix/store/") {
    rest
  } else if path.starts_with("/tmp/") {
    tmp_store_name_from_path(path).ok_or_else(|| {
      eyre!("path '{path}' does not include a valid temporary store-path name")
    })?
  } else {
    return Err(eyre!(
      "path '{path}' does not match expected Nix store format"
    ));
  };

  validate_store_name(path, store_name)
}

fn tmp_store_name_from_path(path: &str) -> Option<&str> {
  let mut search_start = "/tmp/".len();

  while let Some(relative_slash) = path[search_start..].find('/') {
    let hash_start = search_start + relative_slash + 1;
    let candidate = &path[hash_start..];
    if store_hash_prefix_len(candidate).is_some() {
      return Some(candidate);
    }
    search_start = hash_start;
  }

  None
}

fn validate_store_name<'a>(path: &str, store_name: &'a str) -> Result<&'a str> {
  let Some((hash, name)) = store_name.split_once('-') else {
    return Err(eyre!(
      "path '{path}' does not include a Nix store hash separator"
    ));
  };

  if hash.len() != 32 || store_hash_prefix_len(store_name).is_none() {
    return Err(eyre!(
      "path '{path}' does not include a valid Nix store hash"
    ));
  }

  Ok(name)
}

fn store_hash_prefix_len(store_name: &str) -> Option<usize> {
  let bytes = store_name.as_bytes();
  if bytes.len() < 33 || bytes[32] != b'-' {
    return None;
  }

  bytes[..32]
    .iter()
    .all(u8::is_ascii_alphanumeric)
    .then_some(32)
}

fn split_name_and_version(store_name: &str) -> Result<ParsedStorePath> {
  if store_name.is_empty() {
    bail!("failed to extract name from store path");
  }

  let version_start =
    store_name
      .as_bytes()
      .windows(2)
      .enumerate()
      .find_map(|(index, window)| {
        (index > 0 && window[0] == b'-' && window[1].is_ascii_digit())
          .then_some(index)
      });

  let Some(version_start) = version_start else {
    return Ok(ParsedStorePath {
      name:    store_name.to_owned(),
      version: None,
    });
  };

  Ok(ParsedStorePath {
    name:    store_name[..version_start].to_owned(),
    version: Some(Version::from(store_name[version_start + 1..].to_owned())),
  })
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
    fn parses_valid_paths(s in r"((/nix/store/)|(/tmp/.+?/))[a-z0-9A-Z]{32}-.+([0-9][-a-z0-9A-Z\.]*)?") {
      let path = PathBuf::from(s);
      let store_path = StorePath::try_from(path).expect("Failed to create StorePath");
      assert!(!store_path.package_name().is_empty());
    }
  }

  use super::*;

  #[test]
  fn test_store_path_from_nix_store() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path =
      StorePath::try_from(path.clone()).expect("Failed to create StorePath");
    assert_eq!(store_path.as_ref(), path.as_path());
  }

  #[test]
  fn test_store_path_from_tmp_file() {
    let path =
      PathBuf::from("/tmp/test123/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path =
      StorePath::try_from(path.clone()).expect("Failed to create StorePath");
    assert_eq!(store_path.as_ref(), path.as_path());
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
    let store_path =
      StorePath::try_from(path).expect("Failed to create StorePath");
    assert_eq!(store_path.package_name(), "foo");
    assert_eq!(store_path.package_version(), Some(&Version::new("1.0")));
  }
  #[test]
  fn test_name_and_version_parsing_store_path() {
    let path =
      PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo-1.0");
    let store_path =
      StorePath::try_from(path).expect("Failed to create StorePath");
    assert_eq!(store_path.package_name(), "foo");
    assert_eq!(store_path.package_version(), Some(&Version::new("1.0")));
  }

  #[test]
  fn test_name_and_version_parsing_invalid_prefix() {
    let path =
      PathBuf::from("/nix/store/-0123456789abcdefghijklmnopqrstuv-foo-1.0");
    assert!(StorePath::try_from(path).is_err());
  }

  #[test]
  fn test_name_and_version_parsing_no_version() {
    let path = PathBuf::from("/nix/store/0123456789abcdefghijklmnopqrstuv-foo");
    let store_path = StorePath::try_from(path).unwrap();
    assert_eq!(store_path.package_name(), "foo");
    assert_eq!(store_path.package_version(), None);
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
      assert!(!store_path.package_name().is_empty());
    }
  }

  /// returns a temporary directory path (or creates it)
  fn get_temp_dir() -> &'static Path {
    static TEMP_DIR: OnceLock<TempDir> = OnceLock::new();
    TEMP_DIR
      .get_or_init(|| {
        TempDir::new().expect("Failed to create temporary directory.")
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
