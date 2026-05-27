use std::{
  cmp,
  fmt::{
    self,
    Write as _,
  },
};

use itertools::{
  EitherOrBoth,
  Itertools,
};
use unicode_width::UnicodeWidthStr as _;
use yansi::{
  Paint as _,
  Painted,
};

use crate::{
  Version,
  matching::match_version_lists,
  model::{
    Change,
    DerivationSelectionStatus,
    Diff,
    DiffStatus,
  },
  version::VersionPiece,
};

pub(crate) fn render_package_diffs(
  writer: &mut impl fmt::Write,
  diffs: &[Diff],
) -> Result<usize, fmt::Error> {
  let mut diffs = diffs.iter().collect::<Vec<_>>();
  diffs
    .sort_by(|a, b| a.status.cmp(&b.status).then_with(|| a.name.cmp(&b.name)));

  render_diffs(writer, &diffs)
}

fn render_diffs(
  writer: &mut impl fmt::Write,
  diffs: &[&Diff],
) -> Result<usize, fmt::Error> {
  let name_width = diffs
    .iter()
    .map(|diff| diff.name.width())
    .max()
    .unwrap_or(0)
    + 1;
  let mut last_status = None::<DiffStatus>;

  for diff in diffs {
    if last_status
      .is_none_or(|status| status.cmp(&diff.status) != cmp::Ordering::Equal)
    {
      if last_status.is_some() {
        writeln!(writer)?;
      }

      let header = match diff.status {
        DiffStatus::Changed(_) => "CHANGED",
        DiffStatus::Added => "ADDED",
        DiffStatus::Removed => "REMOVED",
      }
      .bold();

      writeln!(writer, "{header}")?;
      last_status = Some(diff.status);
    }

    let status_char = status_char(diff.status);
    let selection_char = selection_char(diff.selection);
    let name_painted = diff.name.paint(selection_char.style);

    write!(
      writer,
      "[{status_char}{selection_char}] {name_painted:<name_width$}"
    )?;

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

fn status_char(status: DiffStatus) -> Painted<&'static char> {
  match status {
    DiffStatus::Changed(Change::UpgradeDowngrade) => 'C'.yellow().bold(),
    DiffStatus::Changed(Change::Upgraded) => 'U'.bright_cyan().bold(),
    DiffStatus::Changed(Change::Downgraded) => 'D'.magenta().bold(),
    DiffStatus::Added => 'A'.green().bold(),
    DiffStatus::Removed => 'R'.red().bold(),
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
  old_versions: &[Version],
  new_versions: &[Version],
  has_common_versions: bool,
) -> Result<(String, String), fmt::Error> {
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
          write_version_piece(&mut old_acc, &comp, |value| value.red())?;
        }
      },
      EitherOrBoth::Right(new) => {
        append_sep(&mut new_acc, &mut new_wrote)?;
        for comp in new {
          write_version_piece(&mut new_acc, &comp, |value| value.green())?;
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

  if old_ver.amount == new_ver.amount {
    if old_ver.amount > 1 {
      write!(old_acc, " ×{}", old_ver.amount.to_string().yellow())?;
      write!(new_acc, " ×{}", new_ver.amount.to_string().yellow())?;
    }
  } else {
    if old_ver.amount > 1 {
      write!(old_acc, " ×{}", old_ver.amount.to_string().red())?;
    }
    if new_ver.amount > 1 {
      write!(new_acc, " ×{}", new_ver.amount.to_string().green())?;
    }
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
