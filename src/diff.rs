use std::{
  cmp::{
    self,
    min,
  },
  collections::{
    HashMap,
    HashSet,
  },
  fmt::{
    self,
    Write as _,
  },
  mem::swap,
  path::{
    Path,
    PathBuf,
  },
  thread,
};

use ::std::hash::BuildHasher;
use eyre::{
  Error,
  Result,
  WrapErr as _,
};
use itertools::{
  EitherOrBoth,
  Itertools,
};
use pathfinding::{
  kuhn_munkres,
  matrix::Matrix,
};
#[cfg(feature = "json")] use serde::Serialize;
use size::Size;
use unicode_width::UnicodeWidthStr as _;
use yansi::{
  Paint as _,
  Painted,
};

use crate::{
  StorePath,
  Version,
  snapshot::{
    SnapshotDiffReport,
    diff_snapshots,
    gather_snapshot,
  },
  store::{
    self,
    StoreBackend,
  },
  version::{
    VersionComponent,
    VersionPiece,
  },
};

pub(crate) fn create_backend<'a>(
  force_correctness: bool,
) -> store::CombinedStoreBackend<'a> {
  if force_correctness {
    store::CombinedStoreBackend::default_eager()
  } else {
    store::CombinedStoreBackend::default_lazy()
  }
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub struct Diff<T = Vec<Version>> {
  pub name:                String,
  pub old:                 T,
  pub new:                 T,
  pub status:              DiffStatus,
  pub selection:           DerivationSelectionStatus,
  pub has_common_versions: bool,
}

impl<T> Default for Diff<T>
where
  T: Default,
{
  fn default() -> Self {
    Self {
      name:                String::default(),
      old:                 T::default(),
      new:                 T::default(),
      status:              DiffStatus::Changed(Change::UpgradeDowngrade),
      selection:           DerivationSelectionStatus::Unselected,
      has_common_versions: false,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum Change {
  UpgradeDowngrade,
  Upgraded,
  Downgraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum DiffStatus {
  Changed(Change),
  Added,
  Removed,
}

impl DiffStatus {
  fn char(self) -> Painted<&'static char> {
    match self {
      Self::Changed(Change::UpgradeDowngrade) => 'C'.yellow().bold(),
      Self::Changed(Change::Upgraded) => 'U'.bright_cyan().bold(),
      Self::Changed(Change::Downgraded) => 'D'.magenta().bold(),
      Self::Added => 'A'.green().bold(),
      Self::Removed => 'R'.red().bold(),
    }
  }
}

impl PartialOrd for DiffStatus {
  fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl cmp::Ord for DiffStatus {
  fn cmp(&self, other: &Self) -> cmp::Ordering {
    // Define a consistent ordering:
    // Changed comes first, then Added, then Removed
    #[expect(clippy::pattern_type_mismatch, clippy::match_same_arms)]
    match (self, other) {
      // Same variants are equal
      (Self::Changed(_), Self::Changed(_)) => cmp::Ordering::Equal,
      (Self::Added, Self::Added) => cmp::Ordering::Equal,
      (Self::Removed, Self::Removed) => cmp::Ordering::Equal,

      // Changed comes before everything else
      (Self::Changed(_), _) => cmp::Ordering::Less,
      (_, Self::Changed(_)) => cmp::Ordering::Greater,

      // Added comes before Removed
      (Self::Added, Self::Removed) => cmp::Ordering::Less,
      (Self::Removed, Self::Added) => cmp::Ordering::Greater,
    }
  }
}

/// Documents if the derivation is a system package and if
/// it was added / removed as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
#[cfg_attr(feature = "json", derive(Serialize))]
pub enum DerivationSelectionStatus {
  /// The derivation is a system package, status unchanged.
  Selected,
  /// The derivation was not a system package before but is now.
  NewlySelected,
  /// The derivation is and was a dependency.
  Unselected,
  /// The derivation was a system package before but is not anymore.
  NewlyUnselected,
}

impl DerivationSelectionStatus {
  fn from_names(
    name: &str,
    old: &HashSet<String>,
    new: &HashSet<String>,
  ) -> Self {
    match (old.contains(name), new.contains(name)) {
      (true, true) => Self::Selected,
      (true, false) => Self::NewlyUnselected,
      (false, true) => Self::NewlySelected,
      (false, false) => Self::Unselected,
    }
  }

  fn char(self) -> Painted<&'static char> {
    match self {
      Self::Selected => '*'.bold(),
      Self::NewlySelected => '+'.bold(),
      Self::Unselected => Painted::new(&'.'),
      Self::NewlyUnselected => Painted::new(&'-'),
    }
  }
}

/// Computes a package diff report between two paths.
///
/// This gathers detached closure snapshots from the configured backend and
/// then reuses `diff_snapshots(...)` for the semantic package diffing step.
pub fn package_diff_report(
  path_old: &Path,
  path_new: &Path,
  force_correctness: bool,
) -> Result<SnapshotDiffReport> {
  tracing::debug!(
    old_path = %path_old.display(),
    new_path = %path_new.display(),
    force_correctness = force_correctness,
    "starting package diff computation"
  );
  let mut connection = create_backend(force_correctness);
  connection.connect()?;

  let report = (|| {
    let old = gather_snapshot(path_old, &connection)
      .with_context(|| format!("failed to gather snapshot for '{}'", path_old.display()))?;
    let new = gather_snapshot(path_new, &connection)
      .with_context(|| format!("failed to gather snapshot for '{}'", path_new.display()))?;
    diff_snapshots(&old, &new)
  })();

  connection.close()?;
  report
}

/// Writes a package diff between two paths to the provided writer.
///
/// This gathers detached closure snapshots from the configured backend and
/// renders the resulting semantic package diff.
///
/// # Returns
///
/// Returns the number of package diffs written.
///
/// # Errors
///
/// Returns an error if:
/// - Failed to connect to the store
/// - Failed to gather snapshots
/// - Failed to write to the output
pub fn write_package_diff(
  writer: &mut impl fmt::Write,
  path_old: &Path,
  path_new: &Path,
  force_correctness: bool,
) -> Result<usize> {
  let report = package_diff_report(path_old, path_new, force_correctness)?;

  writeln!(writer)?;
  let count = render_diffs(writer, &report.diffs).map_err(Error::from);
  tracing::info!(diff_count = ?count.as_ref().ok(), "package diff complete");
  count
}

/// Computes the Levenshtein distance between two slices.
fn levenshtein<T: Eq>(from: &[T], to: &[T]) -> usize {
  let (from_len, to_len) = (from.len(), to.len());

  if from_len == 0 {
    return to_len;
  }
  if to_len == 0 {
    return from_len;
  }

  // Use 'from' as the shorter slice for memory efficiency
  let (from, to, from_len, to_len) = if from_len > to_len {
    (to, from, to_len, from_len)
  } else {
    (from, to, from_len, to_len)
  };

  let mut prev: Vec<usize> = (0..=to_len).collect();
  let mut curr = vec![0; to_len + 1];

  for i in 1..=from_len {
    curr[0] = i;
    for j in 1..=to_len {
      let cost = usize::from(from[i - 1] != to[j - 1]);
      curr[j] = min(min(curr[j - 1] + 1, prev[j] + 1), prev[j - 1] + cost);
    }
    swap(&mut prev, &mut curr);
  }

  prev[to_len]
}

/// Takes two lists of versions and tries to match them using the Hungarian
/// algorithm. The matching attempts to minimize the edit distance between
/// version pairs, which means:
///
/// 1. Versions with minimal edit distance are paired
/// 2. The natural ordering of versions is preserved where possible
///
/// Returns a vector of paired or unpaired versions (as `EitherOrBoth` enum).
pub fn match_version_lists<'a>(
  mut from: &'a [Version],
  mut to: &'a [Version],
) -> Vec<EitherOrBoth<&'a Version>> {
  // Early return for empty inputs
  if from.is_empty() {
    return to.iter().map(EitherOrBoth::Right).collect();
  }
  if to.is_empty() {
    return from.iter().map(EitherOrBoth::Left).collect();
  }

  // Quick path for common case - exact match
  if from.len() == 1 && to.len() == 1 && from[0] == to[0] {
    return vec![EitherOrBoth::Both(&from[0], &to[0])];
  }

  // Hungarian algorithm requires #rows <= #columns
  // Since the edit distance is symmetric, we can swap inputs if needed
  let swapped = if from.len() > to.len() {
    (to, from) = (from, to);
    true
  } else {
    false
  };

  // Pre-extract version components to avoid repetitive extraction
  let from_components: Vec<Vec<VersionComponent>> = from
    .iter()
    .map(|version| {
      version
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let to_components: Vec<Vec<VersionComponent>> = to
    .iter()
    .map(|version| {
      version
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let mut distances = Matrix::new(from.len(), to.len(), 0_i32);

  // Compute all distances directly into the matrix
  for i in 0..from.len() {
    for j in 0..to.len() {
      distances[(i, j)] =
        i32::try_from(levenshtein(&from_components[i], &to_components[j]))
          .unwrap_or_else(|err| {
            tracing::warn!("Distance must fit in i32: {err}");
            i32::MAX
          });
    }
  }

  // Apply Hungarian algorithm to find optimal pairings
  let (_cost, matchings) =
    kuhn_munkres::kuhn_munkres_min::<i32, Matrix<i32>>(&distances);

  // Process matched pairs
  let mut remaining = (0..to.len()).collect::<HashSet<usize>>();
  let mut pairings =
    Vec::<EitherOrBoth<&Version>>::with_capacity(from.len() + to.len());

  for (i, j) in matchings.into_iter().enumerate() {
    pairings.push(EitherOrBoth::Both(&from[i], &to[j]));
    remaining.remove(&j);
  }

  // Add unmatched items from 'to' list
  if !remaining.is_empty() {
    let mut remaining = remaining.iter().map(|&j| &to[j]).collect::<Vec<_>>();
    remaining.sort_unstable();
    pairings.extend(remaining.into_iter().map(EitherOrBoth::Right));
  }

  // Restore original ordering if we swapped the inputs
  if swapped {
    pairings = pairings.into_iter().map(EitherOrBoth::flip).collect();
  }

  pairings
}

/// Counts versions using a `HashMap`.
fn count_versions(versions: Vec<Version>) -> HashMap<Version, usize> {
  let mut counts = HashMap::new();
  for v in versions {
    *counts.entry(v).or_insert(0) += 1;
  }
  counts
}

/// Entry point for writing package differences.
///
/// # Errors
///
/// Returns an error if it fails writing to the `writer`
pub fn write_packages_diff(
  writer: &mut impl fmt::Write,
  paths_old: impl Iterator<Item = StorePath>,
  paths_new: impl Iterator<Item = StorePath>,
  system_paths_old: impl Iterator<Item = StorePath>,
  system_paths_new: impl Iterator<Item = StorePath>,
) -> Result<usize, fmt::Error> {
  let paths_map = collect_path_versions(paths_old, paths_new);

  let sys_old_set: HashSet<String> = system_paths_old
    .filter_map(|p| p.parse_name_and_version().ok().map(|(n, _)| n.into()))
    .collect();

  let sys_new_set: HashSet<String> = system_paths_new
    .filter_map(|p| p.parse_name_and_version().ok().map(|(n, _)| n.into()))
    .collect();

  let mut diffs = generate_diffs_from_paths(paths_map);
  add_selection_status(&mut diffs, &sys_old_set, &sys_new_set);

  diffs
    .sort_by(|a, b| a.status.cmp(&b.status).then_with(|| a.name.cmp(&b.name)));

  render_diffs(writer, &diffs)
}

/// Collects package names from system paths
///
/// Takes an iterator of store paths and extracts the package names,
/// filtering out any that cannot be parsed. Logs warnings for parse failures.
pub(crate) fn collect_system_names(
  paths: impl Iterator<Item = StorePath>,
  context: &str,
) -> HashSet<String> {
  paths
    .filter_map(|path| {
      match path.parse_name_and_version() {
        Ok((name, _)) => Some(name.into()),
        Err(error) => {
          tracing::warn!("error parsing {context} system path name: {error}");
          None
        },
      }
    })
    .collect()
}

/// Collects and organizes versions from old and new paths
///
/// Creates a mapping from package names to their versions in old and new paths.
/// For each package, stores a tuple of (`old_versions`, `new_versions`).
/// Handles parsing errors by logging warnings and skipping problematic entries.
pub(crate) fn collect_path_versions(
  old: impl Iterator<Item = StorePath>,
  new: impl Iterator<Item = StorePath>,
) -> HashMap<String, (Vec<Version>, Vec<Version>)> {
  let mut paths: HashMap<String, (Vec<Version>, Vec<Version>)> = HashMap::new();
  let mut old_count = 0usize;
  let mut new_count = 0usize;

  for path in old {
    old_count += 1;
    if let Ok((name, version)) = path.parse_name_and_version() {
      tracing::trace!(name = name, version = ?version, "collected old path");
      paths
        .entry(name.into())
        .or_default()
        .0
        .push(version.unwrap_or_else(|| Version::from("<none>".to_owned())));
    } else {
      tracing::warn!(
        path = %path.display(),
        "failed to parse name and version from old path"
      );
    }
  }

  for path in new {
    new_count += 1;
    if let Ok((name, version)) = path.parse_name_and_version() {
      tracing::trace!(name = name, version = ?version, "collected new path");
      paths
        .entry(name.into())
        .or_default()
        .1
        .push(version.unwrap_or_else(|| Version::from("<none>".to_owned())));
    } else {
      tracing::warn!(
        path = %path.display(),
        "failed to parse name and version from new path"
      );
    }
  }

  tracing::debug!(
    old_count = old_count,
    new_count = new_count,
    unique_packages = paths.len(),
    "collected paths"
  );

  paths
}

/// Renders a collection of diffs to the writer
///
/// Formats and writes the diffs in sections (CHANGED, ADDED, REMOVED),
/// including status indicators, package names, and version differences.
///
/// Returns the number of diffs rendered on success.
fn render_diffs(
  writer: &mut impl fmt::Write,
  diffs: &[Diff],
) -> Result<usize, fmt::Error> {
  // Calculate width needed for aligning package names
  let name_width = diffs
    .iter()
    .map(|diff| diff.name.width())
    .max()
    .unwrap_or(0)
    + 1;
  let mut last_status = None::<DiffStatus>;

  for diff in diffs {
    // Print section header when status changes
    if last_status.is_none_or(|ls| ls.cmp(&diff.status) != cmp::Ordering::Equal)
    {
      // Add blank line between sections (except before first section)
      if last_status.is_some() {
        writeln!(writer)?;
      }

      // Format and write the section header
      let header = match diff.status {
        DiffStatus::Changed(_) => "CHANGED",
        DiffStatus::Added => "ADDED",
        DiffStatus::Removed => "REMOVED",
      }
      .bold();

      writeln!(writer, "{header}")?;
      last_status = Some(diff.status);
    }

    // Format package info with status indicators
    let status_char = diff.status.char();
    let sel_char = diff.selection.char();
    let name_painted = diff.name.paint(sel_char.style);

    // Write package name with indicators
    write!(
      writer,
      "[{status_char}{sel_char}] {name_painted:<name_width$}"
    )?;

    // Format and write version differences
    let (old_str, new_str) =
      fmt_version_diffs(&diff.old, &diff.new, diff.has_common_versions)?;
    let arrow = if !old_str.is_empty() && !new_str.is_empty() {
      " -> "
    } else {
      ""
    };
    writeln!(writer, "{old_str}{arrow}{new_str}")?;
  }

  Ok(diffs.len())
}

/// Generates the colored strings for the old and new versions.
///
/// This function:
/// 1. Matches old and new versions using the Hungarian algorithm
/// 2. For each matched pair, formats the differences with appropriate colors
/// 3. Handles unmatched versions in either list
///
/// Returns a tuple of formatted strings for the old and new versions.
fn fmt_version_diffs(
  old_versions: &[Version],
  new_versions: &[Version],
  has_common_versions: bool,
) -> Result<(String, String), fmt::Error> {
  // Pre-allocate strings with reasonable capacity
  let mut old_acc = String::with_capacity(
    old_versions
      .iter()
      .fold(0, |acc, version| acc + version.name.len() + 2),
  );
  let mut new_acc = String::with_capacity(
    new_versions
      .iter()
      .fold(0, |acc, version| acc + version.name.len() + 2),
  );

  let mut old_wrote = false;
  let mut new_wrote = false;

  // Helper function to append comma separators when needed
  let append_sep = |acc: &mut String, wrote: &mut bool| {
    if *wrote {
      write!(acc, ", ")
    } else {
      *wrote = true;
      Ok(())
    }
  };

  #[expect(clippy::redundant_closure_for_method_calls)]
  for diff in match_version_lists(old_versions, new_versions) {
    match diff {
      EitherOrBoth::Left(old) => {
        append_sep(&mut old_acc, &mut old_wrote)?;
        for comp in old {
          write_version_piece(&mut old_acc, &comp, |c| c.red())?;
        }
      },
      EitherOrBoth::Right(new) => {
        append_sep(&mut new_acc, &mut new_wrote)?;
        for comp in new {
          write_version_piece(&mut new_acc, &comp, |c| c.green())?;
        }
      },
      EitherOrBoth::Both(old, new) => {
        if old == new {
          continue;
        }

        append_sep(&mut old_acc, &mut old_wrote)?;
        append_sep(&mut new_acc, &mut new_wrote)?;

        fmt_single_version_diff(&mut old_acc, &mut new_acc, old, new)?;
      },
    }
  }
  if has_common_versions {
    let others_str = "<others>".blue().italic().to_string();
    append_sep(&mut old_acc, &mut old_wrote)?;
    append_sep(&mut new_acc, &mut new_wrote)?;
    write!(old_acc, "{others_str}")?;
    write!(new_acc, "{others_str}")?;
  }

  Ok((old_acc, new_acc))
}

/// Writes a version piece to a string buffer with the specified styling.
///
/// Components (like version numbers) get styled according to the provided style
/// function. Separators (like dots, dashes) are written as-is without styling.
///
/// # Parameters
/// * `buf` - The string buffer to write to
/// * `piece` - The version piece to write
/// * `style` - A function that applies a style to the version component
fn write_version_piece(
  buf: &mut String,
  piece: &VersionPiece,
  style: impl Fn(Painted<&str>) -> Painted<&str>,
) -> fmt::Result {
  match *piece {
    VersionPiece::Component(component) => {
      write!(buf, "{}", style(Painted::new(*component)))
    },
    VersionPiece::Separator(separator) => write!(buf, "{separator}"),
  }
}

/// Handles the logic of comparing two specific versions:
/// 1. Finds common prefixes and suffixes, which are colored yellow.
/// 2. Compares the remaining middle parts, with removals in red and additions
///    in green.
fn fmt_single_version_diff(
  old_acc: &mut String,
  new_acc: &mut String,
  old_ver: &Version,
  new_ver: &Version,
) -> fmt::Result {
  // Process version differences
  // Convert versions to piece vectors
  let old_parts: Vec<_> = old_ver.into_iter().collect();
  let new_parts: Vec<_> = new_ver.into_iter().collect();

  // Early return for empty versions or identical versions with same amounts
  if (old_parts.is_empty() && new_parts.is_empty()) || (old_ver == new_ver) {
    return Ok(());
  }

  // Find common prefix length
  let prefix_len = old_parts
    .iter()
    .zip(&new_parts)
    .take_while(|&(old_part, new_part)| old_part == new_part)
    .count();

  // Get remaining parts after removing the common prefix
  let old_remainder = &old_parts[prefix_len..];
  let new_remainder = &new_parts[prefix_len..];

  // Find common suffix length (if there's anything left after prefix removal)
  let suffix_len = if !old_remainder.is_empty() && !new_remainder.is_empty() {
    old_remainder
      .iter()
      .rev()
      .zip(new_remainder.iter().rev())
      .take_while(|&(old_part, new_part)| old_part == new_part)
      .count()
  } else {
    0
  };

  // Get the three sections: prefix, diff, and suffix
  let prefix = &old_parts[..prefix_len];
  let old_diff_end = old_parts.len() - suffix_len;
  let new_diff_end = new_parts.len() - suffix_len;

  let old_diff = &old_parts[prefix_len..old_diff_end];
  let new_diff = &new_parts[prefix_len..new_diff_end];
  let suffix = if suffix_len > 0 {
    &old_parts[old_diff_end..]
  } else {
    &[]
  };

  // Write common prefix (yellow)
  #[expect(clippy::redundant_closure_for_method_calls)]
  for piece in prefix {
    write_version_piece(old_acc, piece, |c| c.yellow())?;
    write_version_piece(new_acc, piece, |c| c.yellow())?;
  }

  // Write differing middle parts (red/green)
  for pair in Itertools::zip_longest(old_diff.iter(), new_diff.iter()) {
    #[expect(clippy::redundant_closure_for_method_calls)]
    match pair {
      EitherOrBoth::Left(old) => {
        write_version_piece(old_acc, old, |c| c.red())?;
      },
      EitherOrBoth::Right(new) => {
        write_version_piece(new_acc, new, |c| c.green())?;
      },
      EitherOrBoth::Both(old, new) => {
        fmt_version_piece_pair(old_acc, new_acc, old, new)?;
      },
    }
  }

  // Process common suffix
  // Write common suffix (yellow)
  #[expect(clippy::redundant_closure_for_method_calls)]
  for piece in suffix {
    write_version_piece(old_acc, piece, |c| c.yellow())?;
    write_version_piece(new_acc, piece, |c| c.yellow())?;
  }

  // Handle version amount differences
  if old_ver.amount == new_ver.amount {
    if old_ver.amount > 1 {
      // Same amount and greater than 1, display in yellow for both
      write!(old_acc, " ×{}", (old_ver.amount.to_string().yellow()))?;
      write!(new_acc, " ×{}", (new_ver.amount.to_string().yellow()))?;
    }
  } else {
    // Different amounts
    if old_ver.amount > 1 {
      write!(old_acc, " ×{}", (old_ver.amount.to_string().red()))?;
    }
    if new_ver.amount > 1 {
      write!(new_acc, " ×{}", (new_ver.amount.to_string().green()))?;
    }
  }

  Ok(())
}

/// Compares and formats two `VersionPieces`.
/// Format a pair of version pieces for diff display
///
/// For components, performs character-level diffing with special handling for
/// hashes. For separators or mixed types, simply colors them red/green.
/// Compares and formats two `VersionPieces` for displaying differences.
///
/// This function implements specialized character-by-character diffing for
/// version components with special handling for hash-like strings (like Nix
/// package hashes). For separators or mixed types, it simply colors the
/// old piece red and the new piece green.
///
/// Performance optimization is applied for very different components to avoid
/// expensive diffing when components are completely different.
fn fmt_version_piece_pair(
  old_acc: &mut String,
  new_acc: &mut String,
  old_piece: &VersionPiece,
  new_piece: &VersionPiece,
) -> fmt::Result {
  // Fast path for identical pieces
  if old_piece == new_piece {
    #[expect(clippy::redundant_closure_for_method_calls)]
    return {
      write_version_piece(old_acc, old_piece, |c| c.yellow())?;
      write_version_piece(new_acc, new_piece, |c| c.yellow())
    };
  }

  match (old_piece, new_piece) {
    // For version components, do character-level diffing
    (&VersionPiece::Component(old_c), &VersionPiece::Component(new_c)) => {
      // Skip detailed diffing for completely different components
      if old_c.len() > 20
        && new_c.len() > 20
        && old_c
          .chars()
          .zip(new_c.chars())
          .all(|(old_char, new_char)| old_char != new_char)
      {
        write!(old_acc, "{}", old_c.red())?;
        write!(new_acc, "{}", new_c.green())?;
        return Ok(());
      }

      let char_diffs = diff::chars(*old_c, *new_c);
      let mut diff_active = false;

      for res in char_diffs {
        match res {
          diff::Result::Both(left, right) => {
            // For matching characters, use yellow unless in hash diff mode
            if diff_active {
              write!(old_acc, "{}", left.red())?;
              write!(new_acc, "{}", right.green())?;
            } else {
              write!(old_acc, "{}", left.yellow())?;
              write!(new_acc, "{}", right.yellow())?;
            }
          },
          diff::Result::Left(left) => {
            // Character only in old version
            diff_active = true;
            write!(old_acc, "{}", left.red())?;
          },
          diff::Result::Right(right) => {
            // Character only in new version
            diff_active = true;
            write!(new_acc, "{}", right.green())?;
          },
        }
      }
    },
    // For separators or mixed types, color them red/green
    #[expect(clippy::redundant_closure_for_method_calls)]
    (old, new) => {
      write_version_piece(old_acc, old, |c| c.red())?;
      write_version_piece(new_acc, new, |c| c.green())?;
    },
  }
  Ok(())
}

/// Spawns a background task to compute the closure sizes required by
/// [`write_size_diff`].
///
/// This function offloads the potentially expensive operation of calculating
/// closure sizes to a separate thread, allowing the main thread to continue
/// with other work while these calculations are performed.
///
/// # Returns
///
/// Returns a join handle that will resolve to the sizes when complete.
#[must_use]
pub fn spawn_size_diff(
  path_old: PathBuf,
  path_new: PathBuf,
  force_correctness: bool,
) -> thread::JoinHandle<Result<(Size, Size)>> {
  tracing::debug!("calculating closure sizes in background");

  thread::spawn(move || {
    let mut connection = create_backend(force_correctness);
    connection.connect()?;

    let result = (
      connection.query_closure_size(&path_old)?,
      connection.query_closure_size(&path_new)?,
    );

    connection.close()?;

    Ok::<_, Error>(result)
  })
}

/// Writes a formatted size difference between two sizes to the provided writer.
///
/// This function displays both the absolute sizes (old → new) and the
/// difference between them, with appropriate coloring (red for size increase,
/// green for size decrease).
///
/// # Returns
///
/// Returns `Ok(())` when successful.
///
/// # Errors
///
/// Returns `Err` when writing to `writer` fails.
pub fn write_size_diff(
  writer: &mut impl fmt::Write,
  size_old: Size,
  size_new: Size,
) -> fmt::Result {
  let size_diff = size_new - size_old;

  writeln!(
    writer,
    "{header}: {size_old} -> {size_new}",
    header = "SIZE".bold(),
    size_old = size_old.red(),
    size_new = size_new.green(),
  )?;

  writeln!(
    writer,
    "{header}: {size_diff}",
    header = "DIFF".bold(),
    size_diff = if size_diff.bytes() > 0 {
      size_diff.green()
    } else {
      size_diff.red()
    },
  )
}

/// Generates diff objects from a mapping of package names to old and new
/// versions.
#[must_use]
pub fn generate_diffs_from_paths<S: BuildHasher>(
  paths: HashMap<String, (Vec<Version>, Vec<Version>), S>,
) -> Vec<Diff> {
  let mut result = Vec::with_capacity(paths.len());

  #[expect(clippy::iter_over_hash_type)]
  for (name, (old_versions, new_versions)) in paths {
    let old_counts = count_versions(old_versions);
    let new_counts = count_versions(new_versions);

    let old_set: HashSet<Version> = old_counts.keys().cloned().collect();
    let new_set: HashSet<Version> = new_counts.keys().cloned().collect();

    let common_count = old_set.intersection(&new_set).count();

    let unique_old: Vec<Version> =
      old_set.difference(&new_set).cloned().collect();
    let unique_new: Vec<Version> =
      new_set.difference(&old_set).cloned().collect();

    let status = if unique_old.is_empty() && unique_new.is_empty() {
      continue;
    } else if common_count == 0 && unique_old.is_empty() {
      DiffStatus::Added
    } else if common_count == 0 && unique_new.is_empty() {
      DiffStatus::Removed
    } else if unique_old.is_empty() || unique_new.is_empty() {
      DiffStatus::Changed(Change::UpgradeDowngrade)
    } else {
      determine_change_status(&unique_old, &unique_new)
        .unwrap_or(DiffStatus::Changed(Change::UpgradeDowngrade))
    };

    result.push(Diff {
      name,
      old: unique_old,
      new: unique_new,
      status,
      selection: DerivationSelectionStatus::Unselected,
      has_common_versions: common_count > 0,
    });
  }

  result
}
/// Determines if changes are upgrades, downgrades, or both.
fn determine_change_status(
  old_versions: &[Version],
  new_versions: &[Version],
) -> Option<DiffStatus> {
  let mut saw_upgrade = false;
  let mut saw_downgrade = false;

  for ver_diff in match_version_lists(old_versions, new_versions) {
    match ver_diff {
      EitherOrBoth::Left(_) => saw_downgrade = true,
      EitherOrBoth::Right(_) => saw_upgrade = true,
      EitherOrBoth::Both(old, new) => {
        match old.cmp(new) {
          cmp::Ordering::Less => saw_upgrade = true,
          cmp::Ordering::Greater => saw_downgrade = true,
          cmp::Ordering::Equal => {},
        }
      },
    }
    if saw_upgrade && saw_downgrade {
      break;
    }
  }

  match (saw_upgrade, saw_downgrade) {
    (true, true) => Some(DiffStatus::Changed(Change::UpgradeDowngrade)),
    (true, false) => Some(DiffStatus::Changed(Change::Upgraded)),
    (false, true) => Some(DiffStatus::Changed(Change::Downgraded)),
    (false, false) => None,
  }
}

/// Adds selection status to each diff based on whether the package name
/// is present in the old and new system paths.
///
/// This determines if packages are system packages and if their status changed,
/// allowing the renderer to show appropriate indicators:
/// - `Selected` (*)      - System package in both old and new
/// - `NewlySelected` (+) - Dependency in old, system package in new
/// - `Unselected` (.)    - Dependency in both old and new
/// - `NewlyUnselected` (-) - System package in old, dependency in new
pub fn add_selection_status(
  diffs: &mut [Diff],
  system_paths_old: &HashSet<String>,
  system_paths_new: &HashSet<String>,
) {
  for diff in diffs {
    diff.selection = DerivationSelectionStatus::from_names(
      &diff.name,
      system_paths_old,
      system_paths_new,
    );
  }
}

#[cfg(test)]
mod tests {
  use proptest::proptest;

  use super::*;

  proptest! {
    #[test]
    fn no_crash_edit_dist(from in r"(\PC-)*(\PC)?", to in r"(\PC-)*(\PC)?") {
      let from = Version::from(from);
      let from: Vec<VersionComponent> = from
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      let to = Version::from(to);
      let to: Vec<VersionComponent> =to
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();


      levenshtein(&*from, &*to);
    }
    #[test]
    fn symmetry_edit_dist(from in r"(\PC-)*(\PC)?", to in r"(\PC-)*(\PC)?") {
      let from = Version::from(from);
      let from: Vec<VersionComponent> = from
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      let to = Version::from(to);
      let to: Vec<VersionComponent> =to
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();


      let forward = levenshtein(&*from, &*to);
      let backward = levenshtein(&*to, &*from);
      assert_eq!(forward, backward);
    }
  }

  #[test]
  fn basic_component_edit_dist() {
    let from = Version::from("foo-123.0-man-pages".to_owned());
    let from: Vec<VersionComponent> = from
      .into_iter()
      .filter_map(VersionPiece::component)
      .collect();

    let to = Version::from("foo-123.4.12-man-pages".to_owned());
    let to: Vec<VersionComponent> =
      to.into_iter().filter_map(VersionPiece::component).collect();

    let dist = levenshtein(&from, &to);
    assert_eq!(dist, 2);
  }

  #[test]
  fn levenshtein_distance_tests() {
    assert_eq!(
      levenshtein(
        &"kitten".chars().collect::<Vec<_>>(),
        &"sitting".chars().collect::<Vec<_>>()
      ),
      3
    );

    assert_eq!(
      levenshtein(
        &"".chars().collect::<Vec<_>>(),
        &"hello".chars().collect::<Vec<_>>()
      ),
      5
    );

    assert_eq!(
      levenshtein(
        &"abcd".chars().collect::<Vec<_>>(),
        &"dcba".chars().collect::<Vec<_>>()
      ),
      4
    );

    assert_eq!(
      levenshtein(
        &"12345".chars().collect::<Vec<_>>(),
        &"12345".chars().collect::<Vec<_>>()
      ),
      0
    );

    assert_eq!(
      levenshtein(
        &"distance".chars().collect::<Vec<_>>(),
        &"difference".chars().collect::<Vec<_>>()
      ),
      5
    );
  }

  #[test]
  fn match_version_lists_test() {
    let version_list_a = [Version::new("6.16.0"), Version::new("5.116.0")];
    let version_list_b = [Version::new("6.17.0"), Version::new("5.116.0-bin")];

    let matched = match_version_lists(&version_list_a, &version_list_b);

    for version in matched {
      #[expect(clippy::print_stdout)]
      match version {
        itertools::EitherOrBoth::Both(left, right) => {
          println!("{left} {right}");
          assert!(
            left == &Version::new("6.16.0") || left == &Version::new("5.116.0")
          );
          assert!(
            right == &Version::new("6.17.0")
              || right == &Version::new("5.116.0-bin")
          );
        },
        itertools::EitherOrBoth::Left(left) => {
          println!("{left}");
          assert!(left == &Version::new("5.116.0-bin"))
        },
        itertools::EitherOrBoth::Right(right) => {
          println!("{right}");
          assert!(right == &Version::new("5.116.0"))
        },
      }
    }
  }

  #[test]
  fn generate_diffs_from_paths_test() {
    let mut paths: HashMap<String, (Vec<Version>, Vec<Version>)> =
      HashMap::new();

    let diff_1 = (vec![Version::new("1.1.0"), Version::new("1.3")], vec![
      Version::new("1.1.0"),
      Version::new("1.4"),
    ]);
    paths.insert("tmp".to_owned(), diff_1);
    let mut vec_1 = generate_diffs_from_paths(paths);
    add_selection_status(
      &mut vec_1,
      &HashSet::<String>::new(),
      &HashSet::<String>::new(),
    );
    let res_2 = Diff {
      name:                "tmp".to_owned(),
      old:                 vec![Version::new("1.3")],
      new:                 vec![Version::new("1.4")],
      status:              DiffStatus::Changed(Change::Upgraded),
      selection:           DerivationSelectionStatus::Unselected,
      has_common_versions: true,
    };
    assert_eq!(vec_1.first().unwrap(), &res_2);

    paths = HashMap::new();

    let diff_2 = (vec![Version::new("1.2.0"), Version::new("1.5")], vec![
      Version::new("1.2.0"),
    ]);
    paths.insert("tmp".to_owned(), diff_2);
    let mut vec_2 = generate_diffs_from_paths(paths);
    add_selection_status(
      &mut vec_2,
      &HashSet::<String>::new(),
      &HashSet::<String>::new(),
    );
    let res_2 = Diff {
      name:                "tmp".to_owned(),
      old:                 vec![Version::new("1.5")],
      new:                 vec![],
      status:              DiffStatus::Changed(Change::UpgradeDowngrade),
      selection:           DerivationSelectionStatus::Unselected,
      has_common_versions: true,
    };
    assert_eq!(vec_2.first().unwrap(), &res_2);
  }

  #[test]
  fn levenshtein_edge_cases() {
    // Both empty
    assert_eq!(levenshtein::<char>(&[], &[]), 0);

    // One empty, one single char
    assert_eq!(levenshtein(&['a'], &[]), 1);
    assert_eq!(levenshtein(&[], &['a']), 1);

    // Single char different
    assert_eq!(levenshtein(&['a'], &['b']), 1);

    // Single char same
    assert_eq!(levenshtein(&['a'], &['a']), 0);

    // Transposition
    assert_eq!(
      levenshtein(
        &"ab".chars().collect::<Vec<_>>(),
        &"ba".chars().collect::<Vec<_>>()
      ),
      2
    );

    // Case sensitivity
    assert_eq!(
      levenshtein(
        &"ABC".chars().collect::<Vec<_>>(),
        &"abc".chars().collect::<Vec<_>>()
      ),
      3
    );

    // Long identical strings
    let long = "a".repeat(1000);
    assert_eq!(
      levenshtein(
        &long.chars().collect::<Vec<_>>(),
        &long.chars().collect::<Vec<_>>()
      ),
      0
    );

    // Long completely different strings
    let long_a = "a".repeat(1000);
    let long_b = "b".repeat(1000);
    assert_eq!(
      levenshtein(
        &long_a.chars().collect::<Vec<_>>(),
        &long_b.chars().collect::<Vec<_>>()
      ),
      1000
    );

    // Unicode characters
    assert_eq!(
      levenshtein(
        &"こんにちは".chars().collect::<Vec<_>>(),
        &"こんばんは".chars().collect::<Vec<_>>()
      ),
      2
    );

    // Substring relationship
    assert_eq!(
      levenshtein(
        &"abc".chars().collect::<Vec<_>>(),
        &"abcabc".chars().collect::<Vec<_>>()
      ),
      3
    );

    // Numbers
    assert_eq!(levenshtein(&[1, 2, 3], &[1, 2, 3, 4, 5]), 2);
  }

  #[test]
  fn match_version_lists_empty() {
    let empty: &[Version] = &[];
    let versions = [Version::new("1.0.0")];

    // Empty left
    let result = match_version_lists(empty, &versions);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], itertools::EitherOrBoth::Right(_)));

    // Empty right
    let result = match_version_lists(&versions, empty);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], itertools::EitherOrBoth::Left(_)));

    // Both empty
    let result = match_version_lists(empty, empty);
    assert!(result.is_empty());
  }

  #[test]
  fn match_version_lists_exact_matches() {
    // Exact same single version
    let a = [Version::new("1.0.0")];
    let b = [Version::new("1.0.0")];
    let result = match_version_lists(&a, &b);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], itertools::EitherOrBoth::Both(_, _)));

    // Exact same multiple versions
    let a = [Version::new("1.0.0"), Version::new("2.0.0")];
    let b = [Version::new("1.0.0"), Version::new("2.0.0")];
    let result = match_version_lists(&a, &b);
    let both_count = result
      .iter()
      .filter(|r| matches!(r, itertools::EitherOrBoth::Both(_, _)))
      .count();
    assert_eq!(both_count, 2);
  }

  #[test]
  fn match_version_lists_unequal_sizes() {
    // More versions on left
    let a = [
      Version::new("1.0.0"),
      Version::new("2.0.0"),
      Version::new("3.0.0"),
    ];
    let b = [Version::new("1.0.0")];
    let result = match_version_lists(&a, &b);
    assert_eq!(result.len(), 3);

    // More versions on right
    let a = [Version::new("1.0.0")];
    let b = [
      Version::new("1.0.0"),
      Version::new("2.0.0"),
      Version::new("3.0.0"),
    ];
    let result = match_version_lists(&a, &b);
    assert_eq!(result.len(), 3);
  }

  #[test]
  fn match_version_lists_similar_versions() {
    // Similar versions should be matched together
    let a = [Version::new("1.0.0"), Version::new("2.0.0")];
    let b = [Version::new("1.0.1"), Version::new("2.0.0")];
    let result = match_version_lists(&a, &b);

    // 2.0.0 should be matched exactly
    let exact_match = result.iter().any(|r| {
      if let itertools::EitherOrBoth::Both(left, right) = r {
        left.name == "2.0.0" && right.name == "2.0.0"
      } else {
        false
      }
    });
    assert!(exact_match);
  }

  #[test]
  fn generate_diffs_empty_paths() {
    let paths: HashMap<String, (Vec<Version>, Vec<Version>)> = HashMap::new();
    let result = generate_diffs_from_paths(paths);
    assert!(result.is_empty());
  }

  #[test]
  fn generate_diffs_unchanged_package() {
    let mut paths = HashMap::new();
    paths.insert(
      "package".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("1.0.0")]),
    );
    let result = generate_diffs_from_paths(paths);
    assert!(result.is_empty()); // No changes, should be filtered out
  }

  #[test]
  fn generate_diffs_added_package() {
    let mut paths = HashMap::new();
    paths.insert("new-pkg".to_owned(), (vec![], vec![Version::new("1.0.0")]));
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "new-pkg");
    assert_eq!(result[0].status, DiffStatus::Added);
    assert!(result[0].old.is_empty());
    assert_eq!(result[0].new.len(), 1);
  }

  #[test]
  fn generate_diffs_removed_package() {
    let mut paths = HashMap::new();
    paths.insert("old-pkg".to_owned(), (vec![Version::new("1.0.0")], vec![]));
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "old-pkg");
    assert_eq!(result[0].status, DiffStatus::Removed);
    assert_eq!(result[0].old.len(), 1);
    assert!(result[0].new.is_empty());
  }

  #[test]
  fn generate_diffs_upgraded() {
    let mut paths = HashMap::new();
    paths.insert(
      "pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].status, DiffStatus::Changed(Change::Upgraded));
    assert_eq!(result[0].old[0].name, "1.0.0");
    assert_eq!(result[0].new[0].name, "2.0.0");
  }

  #[test]
  fn generate_diffs_downgraded() {
    let mut paths = HashMap::new();
    paths.insert(
      "pkg".to_owned(),
      (vec![Version::new("2.0.0")], vec![Version::new("1.0.0")]),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].status, DiffStatus::Changed(Change::Downgraded));
  }

  #[test]
  fn generate_diffs_upgrade_downgrade() {
    let mut paths = HashMap::new();
    // Test with 3 versions: one upgrade, one downgrade
    // Old: 1.0, 5.0
    // New: 2.0, 4.0
    // Matching: 1.0->2.0 (upgrade), 5.0 unmatched (downgrade), 4.0 unmatched
    // (upgrade) Result: UpgradeDowngrade (both types present)
    paths.insert(
      "pkg".to_owned(),
      (vec![Version::new("1.0"), Version::new("5.0")], vec![
        Version::new("2.0"),
        Version::new("4.0"),
      ]),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    // Should detect both upgrade and downgrade
    assert_eq!(
      result[0].status,
      DiffStatus::Changed(Change::UpgradeDowngrade)
    );
  }

  #[test]
  fn generate_diffs_multiple_packages() {
    let mut paths = HashMap::new();
    paths.insert(
      "pkg-a".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );
    paths.insert("pkg-b".to_owned(), (vec![], vec![Version::new("1.0.0")]));
    paths.insert(
    "pkg-c".to_owned(),
    (vec![Version::new("1.0.0")], vec![Version::new("1.0.0")]), // unchanged, filtered
  );

    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 2);

    let names: HashSet<_> = result.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains("pkg-a"));
    assert!(names.contains("pkg-b"));
    assert!(!names.contains("pkg-c"));
  }

  #[test]
  fn generate_diffs_version_deduplication() {
    let mut paths = HashMap::new();
    // Multiple identical versions should be counted
    paths.insert(
      "pkg".to_owned(),
      (
        vec![
          Version::new("1.0.0"),
          Version::new("1.0.0"),
          Version::new("1.0.0"),
        ],
        vec![Version::new("2.0.0")],
      ),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    // Should detect the upgrade from 1.0.0 to 2.0.0
    assert_eq!(result[0].status, DiffStatus::Changed(Change::Upgraded));
  }

  #[test]
  fn generate_diffs_common_versions() {
    let mut paths = HashMap::new();
    paths.insert(
      "pkg".to_owned(),
      (vec![Version::new("1.0.0"), Version::new("2.0.0")], vec![
        Version::new("2.0.0"),
        Version::new("3.0.0"),
      ]),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert!(result[0].has_common_versions);
    assert_eq!(result[0].status, DiffStatus::Changed(Change::Upgraded));
  }

  #[test]
  fn selection_status_selected() {
    let mut paths = HashMap::new();
    paths.insert(
      "system-pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("1.0.0")]),
    );

    let result = generate_diffs_from_paths(paths);
    assert!(result.is_empty()); // No changes

    // Create a changed package for selection testing
    let mut paths = HashMap::new();
    paths.insert(
      "system-pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );

    let mut result = generate_diffs_from_paths(paths);
    let mut sys_old = HashSet::new();
    let mut sys_new = HashSet::new();
    sys_old.insert("system-pkg".to_owned());
    sys_new.insert("system-pkg".to_owned());

    add_selection_status(&mut result, &sys_old, &sys_new);
    assert_eq!(result[0].selection, DerivationSelectionStatus::Selected);
  }

  #[test]
  fn selection_status_newly_selected() {
    let mut paths = HashMap::new();
    paths.insert(
      "new-system-pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );

    let mut result = generate_diffs_from_paths(paths);
    let sys_old = HashSet::new();
    let mut sys_new = HashSet::new();
    sys_new.insert("new-system-pkg".to_owned());

    add_selection_status(&mut result, &sys_old, &sys_new);
    assert_eq!(
      result[0].selection,
      DerivationSelectionStatus::NewlySelected
    );
  }

  #[test]
  fn selection_status_newly_unselected() {
    let mut paths = HashMap::new();
    paths.insert(
      "removed-system-pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );

    let mut result = generate_diffs_from_paths(paths);
    let mut sys_old = HashSet::new();
    let sys_new = HashSet::new();
    sys_old.insert("removed-system-pkg".to_owned());

    add_selection_status(&mut result, &sys_old, &sys_new);
    assert_eq!(
      result[0].selection,
      DerivationSelectionStatus::NewlyUnselected
    );
  }

  #[test]
  fn selection_status_unselected() {
    let mut paths = HashMap::new();
    paths.insert(
      "dep-pkg".to_owned(),
      (vec![Version::new("1.0.0")], vec![Version::new("2.0.0")]),
    );

    let mut result = generate_diffs_from_paths(paths);
    let sys_old = HashSet::new();
    let sys_new = HashSet::new();

    add_selection_status(&mut result, &sys_old, &sys_new);
    assert_eq!(result[0].selection, DerivationSelectionStatus::Unselected);
  }

  #[test]
  fn generate_diffs_many_versions() {
    let mut paths = HashMap::new();
    let old_versions: Vec<_> = (0..100)
      .map(|i| Version::new(format!("1.{i}.{i}")))
      .collect();
    let new_versions: Vec<_> = (50..150)
      .map(|i| Version::new(format!("1.{i}.{i}")))
      .collect();

    paths.insert("large-pkg".to_owned(), (old_versions, new_versions));
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert!(result[0].has_common_versions);
    // Should have 50 old-only and 50 new-only versions
    assert_eq!(result[0].old.len(), 50);
    assert_eq!(result[0].new.len(), 50);
  }

  #[test]
  fn generate_diffs_prerelease_versions() {
    let mut paths = HashMap::new();
    paths.insert(
      "pkg".to_owned(),
      (
        vec![Version::new("1.0.0-alpha"), Version::new("1.0.0-beta")],
        vec![Version::new("1.0.0"), Version::new("1.0.0-rc")],
      ),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    // All versions are different
    assert_eq!(result[0].old.len(), 2);
    assert_eq!(result[0].new.len(), 2);
  }

  #[test]
  fn generate_diffs_complex_version_changes() {
    let mut paths = HashMap::new();
    paths.insert(
      "complex-pkg".to_owned(),
      (
        vec![
          Version::new("1.0.0"),
          Version::new("1.0.1"),
          Version::new("1.1.0"),
          Version::new("2.0.0"),
        ],
        vec![
          Version::new("1.0.1"), // Common
          Version::new("1.1.0"), // Common
          Version::new("1.2.0"),
          Version::new("2.0.0"), // Common
          Version::new("3.0.0"),
        ],
      ),
    );
    let result = generate_diffs_from_paths(paths);
    assert_eq!(result.len(), 1);
    assert!(result[0].has_common_versions);
    // 1.0.0 removed, 1.2.0 and 3.0.0 added
    assert_eq!(result[0].old.len(), 1);
    assert_eq!(result[0].new.len(), 2);
  }
}
