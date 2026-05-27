use std::{
  fmt::{
    self,
    Write as _,
  },
  num::NonZeroUsize,
};

use dix_diff::{
  DiffStatus,
  Version,
  VersionAmount,
  VersionDiff,
  VersionPiece,
};
use itertools::{
  EitherOrBoth,
  Itertools,
};
use size::Size;
use unicode_width::UnicodeWidthStr as _;
use yansi::{
  Paint as _,
  Painted,
};

use crate::{
  DerivationSelectionStatus,
  DiffReport,
  PackageDiff,
};

/// Writes a full diff report to the provided writer.
///
/// # Returns
///
/// Returns the number of package diffs written.
///
/// # Errors
///
/// Returns an error if writing to the output fails.
pub fn write_diff_report(
  writer: &mut impl fmt::Write,
  report: &DiffReport,
) -> Result<usize, fmt::Error> {
  writeln!(writer)?;

  let wrote = render_package_diffs(writer, &report.diffs)?;

  if wrote > 0 {
    writeln!(writer)?;
  }

  write_size_diff(writer, report.size_old, report.size_new)?;

  Ok(wrote)
}

fn render_package_diffs(
  writer: &mut impl fmt::Write,
  diffs: &[PackageDiff],
) -> Result<usize, fmt::Error> {
  let mut diffs = diffs.iter().collect::<Vec<_>>();
  diffs.sort_by(|a, b| {
    status_group(a.status)
      .cmp(&status_group(b.status))
      .then_with(|| a.status.cmp(&b.status))
      .then_with(|| a.name.cmp(&b.name))
  });

  render_diffs(writer, &diffs)
}

fn render_diffs(
  writer: &mut impl fmt::Write,
  diffs: &[&PackageDiff],
) -> Result<usize, fmt::Error> {
  let name_width = diffs
    .iter()
    .map(|diff| diff.name.width())
    .max()
    .unwrap_or(0)
    + 1;
  let mut last_status_group = None::<StatusGroup>;

  for diff in diffs {
    let group = status_group(diff.status);
    if last_status_group.is_none_or(|last_group| last_group != group) {
      if last_status_group.is_some() {
        writeln!(writer)?;
      }

      let header = match group {
        StatusGroup::Changed => "CHANGED",
        StatusGroup::Added => "ADDED",
        StatusGroup::Removed => "REMOVED",
      }
      .bold();

      writeln!(writer, "{header}")?;
      last_status_group = Some(group);
    }

    let status_char = status_char(diff.status);
    let selection_char = selection_char(diff.selection);
    let name_painted = diff.name.paint(selection_char.style);

    write!(
      writer,
      "[{status_char}{selection_char}] {name_painted:<name_width$}"
    )?;

    let (old_str, new_str) =
      fmt_version_diffs(&diff.versions, diff.has_omitted_versions)?;
    let arrow = if !old_str.is_empty() && !new_str.is_empty() {
      " -> "
    } else {
      ""
    };
    writeln!(writer, "{old_str}{arrow}{new_str}")?;
  }

  Ok(diffs.len())
}

fn write_size_diff(
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

fn status_char(status: DiffStatus) -> Painted<&'static char> {
  match status {
    DiffStatus::Changed | DiffStatus::Mixed => 'C'.yellow().bold(),
    DiffStatus::Upgraded => 'U'.bright_cyan().bold(),
    DiffStatus::Downgraded => 'D'.magenta().bold(),
    DiffStatus::Added => 'A'.green().bold(),
    DiffStatus::Removed => 'R'.red().bold(),
  }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StatusGroup {
  Changed,
  Added,
  Removed,
}

const fn status_group(status: DiffStatus) -> StatusGroup {
  match status {
    DiffStatus::Changed
    | DiffStatus::Mixed
    | DiffStatus::Upgraded
    | DiffStatus::Downgraded => StatusGroup::Changed,
    DiffStatus::Added => StatusGroup::Added,
    DiffStatus::Removed => StatusGroup::Removed,
  }
}

fn selection_char(status: DerivationSelectionStatus) -> Painted<&'static char> {
  match status {
    DerivationSelectionStatus::Selected => '*'.bold(),
    DerivationSelectionStatus::NewlySelected => '+'.bold(),
    DerivationSelectionStatus::Unselected => Painted::new(&'.'),
    DerivationSelectionStatus::NewlyUnselected => Painted::new(&'-'),
  }
}

fn fmt_version_diffs(
  version_diffs: &[VersionDiff],
  has_omitted_versions: bool,
) -> Result<(String, String), fmt::Error> {
  let mut old_acc = String::new();
  let mut new_acc = String::new();

  let mut old_wrote = false;
  let mut new_wrote = false;

  let append_sep = |acc: &mut String, wrote: &mut bool| {
    if *wrote {
      write!(acc, ", ")
    } else {
      *wrote = true;
      Ok(())
    }
  };

  #[expect(clippy::redundant_closure_for_method_calls)]
  for diff in version_diffs {
    match diff {
      VersionDiff::Removed(old) => {
        append_sep(&mut old_acc, &mut old_wrote)?;
        write_version_amount(&mut old_acc, old, |value| value.red())?;
      },
      VersionDiff::Added(new) => {
        append_sep(&mut new_acc, &mut new_wrote)?;
        write_version_amount(&mut new_acc, new, |value| value.green())?;
      },
      VersionDiff::Changed { old, new } => {
        if old == new {
          continue;
        }

        append_sep(&mut old_acc, &mut old_wrote)?;
        append_sep(&mut new_acc, &mut new_wrote)?;

        fmt_single_version_diff(
          &mut old_acc,
          &mut new_acc,
          &old.version,
          &new.version,
        )?;
        write_changed_amounts(
          &mut old_acc,
          &mut new_acc,
          old.amount,
          new.amount,
        )?;
      },
      VersionDiff::AmountChanged {
        version,
        old_amount,
        new_amount,
      } => {
        append_sep(&mut old_acc, &mut old_wrote)?;
        append_sep(&mut new_acc, &mut new_wrote)?;

        write_version(&mut old_acc, version, |value| value.yellow())?;
        write_version(&mut new_acc, version, |value| value.yellow())?;
        write_amount_suffix(&mut old_acc, *old_amount, |value| value.red())?;
        write_amount_suffix(&mut new_acc, *new_amount, |value| value.green())?;
      },
    }
  }
  if has_omitted_versions {
    let others_str = "<others>".blue().italic().to_string();
    append_sep(&mut old_acc, &mut old_wrote)?;
    append_sep(&mut new_acc, &mut new_wrote)?;
    write!(old_acc, "{others_str}")?;
    write!(new_acc, "{others_str}")?;
  }

  Ok((old_acc, new_acc))
}

fn write_version_amount(
  buf: &mut String,
  version: &VersionAmount,
  style: impl Copy + Fn(Painted<&str>) -> Painted<&str>,
) -> fmt::Result {
  write_version(buf, &version.version, style)?;
  write_amount_suffix(buf, version.amount, style)
}

fn write_version(
  buf: &mut String,
  version: &Version,
  style: impl Copy + Fn(Painted<&str>) -> Painted<&str>,
) -> fmt::Result {
  for piece in version {
    write_version_piece(buf, &piece, style)?;
  }

  Ok(())
}

fn write_amount_suffix(
  buf: &mut String,
  amount: NonZeroUsize,
  style: impl Fn(Painted<&str>) -> Painted<&str>,
) -> fmt::Result {
  if amount.get() > 1 {
    let amount = amount.get().to_string();
    write!(buf, " ×{}", style(Painted::new(amount.as_str())))?;
  }

  Ok(())
}

#[expect(clippy::redundant_closure_for_method_calls)]
fn write_changed_amounts(
  old_acc: &mut String,
  new_acc: &mut String,
  old_amount: NonZeroUsize,
  new_amount: NonZeroUsize,
) -> fmt::Result {
  if old_amount == new_amount {
    write_amount_suffix(old_acc, old_amount, |value| value.yellow())?;
    write_amount_suffix(new_acc, new_amount, |value| value.yellow())
  } else {
    write_amount_suffix(old_acc, old_amount, |value| value.red())?;
    write_amount_suffix(new_acc, new_amount, |value| value.green())
  }
}

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

fn fmt_single_version_diff(
  old_acc: &mut String,
  new_acc: &mut String,
  old_ver: &Version,
  new_ver: &Version,
) -> fmt::Result {
  let old_parts: Vec<_> = old_ver.into_iter().collect();
  let new_parts: Vec<_> = new_ver.into_iter().collect();

  if (old_parts.is_empty() && new_parts.is_empty()) || (old_ver == new_ver) {
    return Ok(());
  }

  let prefix_len = old_parts
    .iter()
    .zip(&new_parts)
    .take_while(|&(old_part, new_part)| old_part == new_part)
    .count();

  let old_remainder = &old_parts[prefix_len..];
  let new_remainder = &new_parts[prefix_len..];

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

  #[expect(clippy::redundant_closure_for_method_calls)]
  for piece in prefix {
    write_version_piece(old_acc, piece, |value| value.yellow())?;
    write_version_piece(new_acc, piece, |value| value.yellow())?;
  }

  for pair in Itertools::zip_longest(old_diff.iter(), new_diff.iter()) {
    #[expect(clippy::redundant_closure_for_method_calls)]
    match pair {
      EitherOrBoth::Left(old) => {
        write_version_piece(old_acc, old, |value| value.red())?;
      },
      EitherOrBoth::Right(new) => {
        write_version_piece(new_acc, new, |value| value.green())?;
      },
      EitherOrBoth::Both(old, new) => {
        fmt_version_piece_pair(old_acc, new_acc, old, new)?;
      },
    }
  }

  #[expect(clippy::redundant_closure_for_method_calls)]
  for piece in suffix {
    write_version_piece(old_acc, piece, |value| value.yellow())?;
    write_version_piece(new_acc, piece, |value| value.yellow())?;
  }

  Ok(())
}

fn fmt_version_piece_pair(
  old_acc: &mut String,
  new_acc: &mut String,
  old_piece: &VersionPiece,
  new_piece: &VersionPiece,
) -> fmt::Result {
  if old_piece == new_piece {
    #[expect(clippy::redundant_closure_for_method_calls)]
    return {
      write_version_piece(old_acc, old_piece, |value| value.yellow())?;
      write_version_piece(new_acc, new_piece, |value| value.yellow())
    };
  }

  match (old_piece, new_piece) {
    (&VersionPiece::Component(old_c), &VersionPiece::Component(new_c)) => {
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
            if diff_active {
              write!(old_acc, "{}", left.red())?;
              write!(new_acc, "{}", right.green())?;
            } else {
              write!(old_acc, "{}", left.yellow())?;
              write!(new_acc, "{}", right.yellow())?;
            }
          },
          diff::Result::Left(left) => {
            diff_active = true;
            write!(old_acc, "{}", left.red())?;
          },
          diff::Result::Right(right) => {
            diff_active = true;
            write!(new_acc, "{}", right.green())?;
          },
        }
      }
    },
    #[expect(clippy::redundant_closure_for_method_calls)]
    (old, new) => {
      write_version_piece(old_acc, old, |value| value.red())?;
      write_version_piece(new_acc, new, |value| value.green())?;
    },
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use std::num::NonZeroUsize;

  use super::*;

  fn amount(amount: usize) -> NonZeroUsize {
    NonZeroUsize::new(amount)
      .unwrap_or_else(|| panic!("test version amount must be nonzero"))
  }

  #[test]
  fn fmt_version_diffs_formats_added_and_removed_amounts() {
    yansi::disable();

    let version_diffs = [
      VersionDiff::Removed(VersionAmount::new("1.0.0", amount(2))),
      VersionDiff::Added(VersionAmount::new("2.0.0", amount(3))),
    ];

    let (old, new) = fmt_version_diffs(&version_diffs, false).unwrap();

    assert_eq!(old, "1.0.0 ×2");
    assert_eq!(new, "2.0.0 ×3");
  }

  #[test]
  fn fmt_version_diffs_formats_amount_changes() {
    yansi::disable();

    let version_diffs = [VersionDiff::AmountChanged {
      version:    Version::new("1.0.0"),
      old_amount: amount(1),
      new_amount: amount(2),
    }];

    let (old, new) = fmt_version_diffs(&version_diffs, false).unwrap();

    assert_eq!(old, "1.0.0");
    assert_eq!(new, "1.0.0 ×2");
  }
}
