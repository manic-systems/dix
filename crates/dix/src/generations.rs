use std::{
  fs,
  num::NonZeroUsize,
  path::{
    Path,
    PathBuf,
  },
};

use eyre::{
  Context as _,
  Result,
  bail,
  eyre,
};

/// The system profile used when no profile is specified.
pub const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";

#[derive(Debug, PartialEq, Eq)]
pub struct Generation {
  pub number: u64,
  pub path:   PathBuf,
}

/// Finds the existing numbered generation links of a Nix profile.
///
/// Generations are the live `PROFILE-N-link` symlinks beside `profile`.
/// Dangling links are skipped.
///
/// # Returns
///
/// The generations, in numerical order.
///
/// # Errors
///
/// Returns an error if `profile` has no file name or its directory cannot be
/// read.
pub fn generations(profile: &Path) -> Result<Vec<Generation>> {
  let parent = profile
    .parent()
    .filter(|p| !p.as_os_str().is_empty())
    .unwrap_or_else(|| Path::new("."));
  let Some(name) = profile.file_name().filter(|name| !name.is_empty()) else {
    bail!("expected a profile path, got '{}'", profile.display());
  };
  let prefix = format!("{}-", name.to_string_lossy());
  let mut found = Vec::new();

  for entry in fs::read_dir(parent).with_context(|| {
    format!("failed to read profile directory '{}'", parent.display())
  })? {
    let entry = entry?;
    let entry_name = entry.file_name();
    let Some(number) = entry_name
      .to_str()
      .and_then(|name| name.strip_prefix(&prefix))
      .and_then(|suffix| suffix.strip_suffix("-link"))
      .filter(|number| {
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
      })
      .and_then(|number| number.parse::<u64>().ok())
    else {
      continue;
    };
    // Only actual, live generation links are useful to the diff backend.
    let path = entry.path();
    if entry.file_type()?.is_symlink() && path.exists() {
      found.push(Generation { number, path });
    }
  }

  found.sort_by_key(|generation| generation.number);
  Ok(found)
}

/// Which generations of a profile to compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
  /// The given number of transitions leading up to the current generation.
  Latest(NonZeroUsize),
  /// Every existing generation numbered within the inclusive bounds,
  /// regardless of which generation is current.
  Range {
    from: Option<u64>,
    to:   Option<u64>,
  },
}

/// Selects the generations of `profile` covered by `span`.
///
/// # Returns
///
/// At least two generations, in numerical order.
///
/// # Errors
///
/// Returns an error if the generations cannot be read, the current one cannot
/// be found, the range is inverted, or fewer than two generations are
/// selected.
pub fn select_generations(
  profile: &Path,
  span: Span,
) -> Result<Vec<Generation>> {
  let mut selected = generations(profile)?;
  match span {
    Span::Latest(count) => {
      // Read the numbered link instead of its store target: generations can
      // share a store path, and a rollback can select an older
      // generation.
      let current = fs::read_link(profile).wrap_err_with(|| {
        format!(
          "failed to read current profile link '{}'",
          profile.display()
        )
      })?;
      let end = selected
        .iter()
        .position(|generation| {
          generation.path.file_name() == current.file_name()
        })
        .ok_or_else(|| {
          eyre!(
            "profile '{}' does not select an existing numbered generation",
            profile.display()
          )
        })?;
      selected.truncate(end + 1);
      selected.drain(..end.saturating_sub(count.get()));
    },
    Span::Range { from, to } => {
      if let (Some(from), Some(to)) = (from, to)
        && from > to
      {
        bail!("--from {from} is greater than --to {to}");
      }
      selected.retain(|generation| {
        from.is_none_or(|from| generation.number >= from)
          && to.is_none_or(|to| generation.number <= to)
      });
    },
  }
  if selected.len() < 2 {
    bail!(
      "profile '{}' has fewer than two generations in the requested range",
      profile.display()
    );
  }
  Ok(selected)
}

#[cfg(test)]
mod tests {
  use std::os::unix::fs::symlink;

  use tempfile::tempdir;

  use super::*;

  const ALL: Span = Span::Range {
    from: None,
    to:   None,
  };

  #[test]
  fn finds_only_live_generations_in_numeric_order() {
    let dir = tempdir().unwrap();
    let profile = dir.path().join("system");
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    symlink(&target, dir.path().join("system-10-link")).unwrap();
    symlink(&target, dir.path().join("system-2-link")).unwrap();
    symlink(&target, dir.path().join("system-4-link")).unwrap();
    symlink(dir.path().join("missing"), dir.path().join("system-3-link"))
      .unwrap();
    fs::create_dir(dir.path().join("system-5-link")).unwrap();
    symlink(&target, dir.path().join("other-1-link")).unwrap();
    symlink(&target, dir.path().join("system-no-link")).unwrap();
    symlink(&target, dir.path().join("system-6-link-extra")).unwrap();
    symlink("system-10-link", &profile).unwrap();

    let found = generations(&profile).unwrap();
    assert_eq!(found.iter().map(|g| g.number).collect::<Vec<_>>(), vec![
      2, 4, 10
    ]);
    assert_eq!(found[0].path, dir.path().join("system-2-link"));
  }

  #[test]
  fn selects_transitions_ending_at_current_generation_across_gaps() {
    let dir = tempdir().unwrap();
    let profile = dir.path().join("system");
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    for number in [2, 4, 10, 20] {
      symlink(&target, dir.path().join(format!("system-{number}-link")))
        .unwrap();
    }
    // All generations deliberately share a target; current is a rollback.
    symlink("system-10-link", &profile).unwrap();
    let latest = |count| Span::Latest(NonZeroUsize::new(count).unwrap());
    let numbers = |span| {
      select_generations(&profile, span)
        .unwrap()
        .into_iter()
        .map(|g| g.number)
        .collect::<Vec<_>>()
    };
    assert_eq!(numbers(latest(1)), [4, 10]);
    assert_eq!(numbers(latest(2)), [2, 4, 10]);
    assert_eq!(numbers(latest(usize::MAX)), [2, 4, 10]);
    assert_eq!(numbers(ALL), [2, 4, 10, 20]);

    fs::remove_file(&profile).unwrap();
    symlink(dir.path().join("system-2-link"), &profile).unwrap();
    assert!(select_generations(&profile, latest(1)).is_err());
    // Ranges do not depend on the active profile link.
    fs::remove_file(&profile).unwrap();
    assert!(select_generations(&profile, latest(1)).is_err());
    assert_eq!(numbers(ALL), [2, 4, 10, 20]);
    let range = |from, to| Span::Range { from, to };
    assert_eq!(numbers(range(Some(3), Some(10))), [4, 10]);
    assert_eq!(numbers(range(Some(10), None)), [10, 20]);
    assert_eq!(numbers(range(None, Some(4))), [2, 4]);
    // Bounds need not be existing generations, but must select two.
    assert!(select_generations(&profile, range(Some(5), Some(9))).is_err());
    assert!(select_generations(&profile, range(Some(10), Some(4))).is_err());
  }

  #[test]
  fn rejects_missing_predecessors_and_unrecognized_current_links() {
    let dir = tempdir().unwrap();
    let profile = dir.path().join("system");
    assert!(select_generations(&profile, ALL).is_err());
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    symlink(&target, dir.path().join("system-1-link")).unwrap();
    assert!(select_generations(&profile, ALL).is_err());
    symlink(&target, &profile).unwrap();
    assert!(
      select_generations(&profile, Span::Latest(NonZeroUsize::MIN)).is_err()
    );
  }

  #[test]
  fn rejects_missing_directory_and_invalid_profile() {
    let dir = tempdir().unwrap();
    assert!(generations(&dir.path().join("missing/system")).is_err());
    assert!(generations(Path::new("/")).is_err());
  }
}
