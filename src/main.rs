use std::{
  cell::LazyCell,
  env,
  fmt::{
    self,
    Write as _,
  },
  fs,
  io::{
    self,
    BufRead as _,
    IsTerminal as _,
    Write as _,
  },
  num::NonZeroUsize,
  path::{
    Path,
    PathBuf,
  },
};

use dix::{
  DiffReport,
  RenderOptions,
  SnapshotDocument,
  json,
};
use misstep::ResultExt as _;
use pound::{
  Parse,
  ValueEnum,
};
use size::Size;
use yansi::Paint as _;

mod generations;
use generations::{
  SYSTEM_PROFILE,
  Span,
  select_generations,
};

struct WriteFmt<W: io::Write>(W);

impl<W: io::Write> fmt::Write for WriteFmt<W> {
  fn write_str(&mut self, string: &str) -> fmt::Result {
    self.0.write_all(string.as_bytes()).map_err(|_| fmt::Error)
  }
}

/// Diff Nix.
#[derive(Parse)]
struct Cli {
  #[pound(subcommand)]
  command: Command,

  /// Show more log output, repeat for more detail.
  #[pound(short, long, count, global)]
  verbose: u8,

  /// Controls when to use color.
  #[pound(long, default = ColorChoice::Auto, value_name = "WHEN", global)]
  color: ColorChoice,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum ColorChoice {
  /// Color when stdout is a terminal and the environment allows it.
  Auto,
  Always,
  Never,
}

#[derive(Parse, Debug, PartialEq, Eq)]
enum Command {
  /// Show the differences between two store paths or snapshots.
  Diff {
    /// Store path, or JSON file written by `dix snapshot`.
    old_path: PathBuf,
    /// Store path, or JSON file written by `dix snapshot`.
    new_path: PathBuf,

    /// Select the output format to use.
    #[pound(long, default = OutputFormat::Human)]
    output: OutputFormat,

    #[pound(flatten)]
    render: RenderArgs,
  },
  /// Show successive changes ending at the current profile generation.
  Last {
    /// Number of transitions to show.
    #[pound(default = "1")]
    count: NonZeroUsize,

    /// Show every adjacent pair of existing generations in numerical order.
    #[pound(long, conflicts_with = [count, from, to])]
    all: bool,

    /// Show generations numbered from GENERATION onwards.
    #[pound(short, long, value_name = "GENERATION", conflicts_with = count)]
    from: Option<u64>,

    /// Show generations numbered up to GENERATION.
    #[pound(short, long, value_name = "GENERATION", conflicts_with = count)]
    to: Option<u64>,

    /// Profile link to inspect.
    #[pound(long, default = { SYSTEM_PROFILE })]
    profile: PathBuf,

    /// Select the output format to use.
    #[pound(long, default = OutputFormat::Human)]
    output: OutputFormat,

    #[pound(flatten)]
    render: RenderArgs,
  },
  /// Print the closure of a store path as a versioned JSON snapshot.
  Snapshot { path: PathBuf },
}

/// Options for the human-readable output.
#[derive(Parse, Debug, Clone, Copy, PartialEq, Eq)]
struct RenderArgs {
  /// Also show packages whose version did not change.
  #[pound(long)]
  full: bool,

  /// Hide packages whose version did not change if their size changed by
  /// less than SIZE (e.g. 4096, 512KiB, 10MB).
  #[pound(long, value_name = "SIZE", default = "1MiB", parse = parse_size)]
  min_size_delta: Size,
}

impl Default for RenderArgs {
  fn default() -> Self {
    let options = RenderOptions::default();
    Self {
      full:           options.full,
      min_size_delta: options.min_size_delta,
    }
  }
}

impl From<RenderArgs> for RenderOptions {
  fn from(args: RenderArgs) -> Self {
    Self {
      full:           args.full,
      min_size_delta: args.min_size_delta,
    }
  }
}

fn parse_size(value: &str) -> Result<Size, String> {
  let size = value
    .parse::<Size>()
    .map_err(|_| format!("invalid size: {value}"))?;
  if size.bytes() < 0 {
    return Err(format!("size must not be negative: {value}"));
  }
  Ok(size)
}

/// Determines the output format to be used by dix.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
  /// Output in the default dix format highlighting version changes.
  Human,
  /// Display the output as JSON for machine parsing.
  Json,
}

fn main() -> misstep::Result<()> {
  let Cli {
    command,
    verbose,
    color,
  } = Cli::parse();

  yansi::whenever(match color {
    ColorChoice::Auto => yansi::Condition::from(should_style),
    ColorChoice::Always => yansi::Condition::ALWAYS,
    ColorChoice::Never => yansi::Condition::NEVER,
  });

  tracing_subscriber::fmt()
    .with_max_level(match verbose {
      0 => tracing::Level::ERROR,
      1 => tracing::Level::WARN,
      2 => tracing::Level::INFO,
      3 => tracing::Level::DEBUG,
      _ => tracing::Level::TRACE,
    })
    .with_writer(io::stderr)
    .with_ansi(should_style())
    .with_target(false)
    .without_time()
    .init();

  match command {
    Command::Diff {
      old_path,
      new_path,
      output,
      render,
    } => print_diff(&old_path, &new_path, output, render.into()),
    Command::Last {
      count,
      all,
      from,
      to,
      profile,
      output,
      render,
    } => {
      let span = if all || from.is_some() || to.is_some() {
        Span::Range { from, to }
      } else {
        Span::Latest(count)
      };
      print_last(&profile, span, output, render.into())
    },
    Command::Snapshot { path } => print_snapshot(&path),
  }
}

/// Prints the snapshot of `path` as JSON to stdout.
///
/// # Returns
///
/// `Ok(())` once the snapshot is printed.
///
/// # Errors
///
/// Returns an error if the snapshot cannot be queried or written.
fn print_snapshot(path: &Path) -> misstep::Result<()> {
  let document = dix::Store::open().query_snapshot_document(path)?;
  let mut out = io::stdout().lock();
  serde_json::to_writer(&mut out, &document)?;
  writeln!(out)?;
  Ok(())
}

/// Prints the diff between `old_path` and `new_path`, each a store path or
/// snapshot file, to stdout.
///
/// # Returns
///
/// `Ok(())` once the diff is printed.
///
/// # Errors
///
/// Returns an error if a path cannot be loaded as a store path or snapshot, or
/// the output cannot be written.
fn print_diff(
  old_path: &Path,
  new_path: &Path,
  output: OutputFormat,
  options: RenderOptions,
) -> misstep::Result<()> {
  // Only opened if an argument is not a snapshot file.
  let store: LazyCell<dix::Store> = LazyCell::new(dix::Store::open);
  let old = load_snapshot(old_path, &store)?;
  let new = load_snapshot(new_path, &store)?;
  let report = dix::diff_store_snapshots(&old.snapshot, &new.snapshot);
  match output {
    OutputFormat::Human => {
      let hidden = display_diff(&old.path, &new.path, &report, options)?;
      dix::write_hidden_note(&mut io::stdout().lock(), hidden, "--full")?;
      Ok(())
    },
    OutputFormat::Json => {
      Ok(serde_json::to_writer(
        io::stdout(),
        &json::JsonReport::from(&report),
      )?)
    },
  }
}

/// Reads the snapshot in `path` if it is a JSON file, and queries the store
/// path `path` resolves to in `store` otherwise.
///
/// # Returns
///
/// The snapshot read from, or queried for, `path`.
///
/// # Errors
///
/// Returns an error if `path` is a JSON file that is not a valid snapshot, or
/// the store cannot be queried.
fn load_snapshot(
  path: &Path,
  store: &LazyCell<dix::Store>,
) -> misstep::Result<SnapshotDocument> {
  if !is_json_file(path)? {
    return store.query_snapshot_document(path);
  }

  tracing::debug!(path = %path.display(), "reading snapshot");
  let input = fs::read_to_string(path)?;
  SnapshotDocument::from_json(&input)
    .with_context(|| format!("failed to read snapshot '{}'", path.display()))
}

/// Checks whether `path` is a file holding a JSON object, by its first
/// non-whitespace byte.
fn is_json_file(path: &Path) -> io::Result<bool> {
  if !path.is_file() {
    return Ok(false);
  }
  let mut reader = io::BufReader::new(fs::File::open(path)?);
  Ok(reader.fill_buf()?.trim_ascii_start().starts_with(b"{"))
}

/// One transition in the array printed by `dix last --output json`.
#[derive(serde::Serialize)]
struct JsonTransition<'a> {
  /// Number of the older generation.
  old_generation: u64,
  /// Number of the newer generation.
  new_generation: u64,
  /// Generation link of the older generation.
  old_path:       &'a Path,
  /// Generation link of the newer generation.
  new_path:       &'a Path,
  /// Diff from the older to the newer generation.
  report:         json::JsonReport<'a>,
}

/// Prints the diff of each transition between the generations of `profile`
/// selected by `span` to stdout.
///
/// # Returns
///
/// `Ok(())` once all transitions are printed.
///
/// # Errors
///
/// Returns an error if the generations cannot be selected, the store cannot
/// be queried, or the output cannot be written.
fn print_last(
  profile: &Path,
  span: Span,
  output: OutputFormat,
  options: RenderOptions,
) -> misstep::Result<()> {
  let generations = select_generations(profile, span)?;
  let store = dix::Store::open();
  let mut snapshots = generations.iter().map(|generation| {
    store
      .query_snapshot_document(&generation.path)
      .map(|document| document.snapshot)
  });
  let mut reports = Vec::with_capacity(generations.len().saturating_sub(1));
  if let Some(mut old) = snapshots.next().transpose()? {
    for new in snapshots {
      let new = new?;
      reports.push(dix::diff_store_snapshots(&old, &new));
      old = new;
    }
  }
  let transitions = generations.array_windows().zip(&reports);

  match output {
    OutputFormat::Human => {
      let mut hidden = 0;
      for ([old, new], report) in transitions {
        writeln!(
          WriteFmt(io::stdout()),
          "Generation {} -> {}",
          old.number,
          new.number
        )?;
        hidden += display_diff(&old.path, &new.path, report, options)?;
      }
      dix::write_hidden_note(&mut io::stdout().lock(), hidden, "--full")?;
      Ok(())
    },
    OutputFormat::Json => {
      let transitions = transitions
        .map(|([old, new], report)| {
          JsonTransition {
            old_generation: old.number,
            new_generation: new.number,
            old_path:       &old.path,
            new_path:       &new.path,
            report:         json::JsonReport::from(report),
          }
        })
        .collect::<Vec<_>>();
      Ok(serde_json::to_writer(io::stdout(), &transitions)?)
    },
  }
}

/// Renders `report`, the diff between `old_path` and `new_path`, for humans to
/// stdout.
///
/// # Returns
///
/// The number of package diffs hidden by `options`.
///
/// # Errors
///
/// Returns an error if the output cannot be written.
fn display_diff(
  old_path: &Path,
  new_path: &Path,
  report: &DiffReport,
  options: RenderOptions,
) -> misstep::Result<usize> {
  let mut out = WriteFmt(io::stdout());

  tracing::info!("rendering diff report");

  writeln!(
    out,
    "{arrows} {old}",
    arrows = "<<<".bold(),
    old = fs::canonicalize(old_path)
      .unwrap_or_else(|_| old_path.to_path_buf())
      .display(),
  )?;
  writeln!(
    out,
    "{arrows} {new}",
    arrows = ">>>".bold(),
    new = fs::canonicalize(new_path)
      .unwrap_or_else(|_| new_path.to_path_buf())
      .display(),
  )?;

  let hidden = dix::write_diff_report(&mut out, report, options)?;

  tracing::info!("diff report rendered");

  Ok(hidden)
}

// https://bixense.com/clicolors/
fn should_style() -> bool {
  // If NO_COLOR is set and is not empty, don't style.
  if let Some(value) = env::var_os("NO_COLOR")
    && !value.is_empty()
  {
    return false;
  }

  // If CLICOLOR is set and is 0, don't style.
  if let Some(value) = env::var_os("CLICOLOR")
    && value == "0"
  {
    return false;
  }

  // If CLICOLOR_FORCE is set and not 0, always style.
  if let Some(value) = env::var_os("CLICOLOR_FORCE")
    && value != "0"
  {
    return true;
  }

  // Style if it is a terminal.
  io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn diff_subcommand_parses() {
    let cli = Cli::try_parse_from(["diff", "/old", "/new"]).unwrap();
    assert_eq!(cli.command, Command::Diff {
      old_path: PathBuf::from("/old"),
      new_path: PathBuf::from("/new"),
      output:   OutputFormat::Human,
      render:   RenderArgs::default(),
    });
  }

  #[test]
  fn bare_paths_are_rejected() {
    assert!(Cli::try_parse_from(["/old", "/new"]).is_err());
  }

  #[test]
  fn last_subcommand_parses() {
    let cli = Cli::try_parse_from(["last", "3"]).unwrap();
    assert_eq!(cli.command, Command::Last {
      count:   NonZeroUsize::new(3).unwrap(),
      all:     false,
      from:    None,
      to:      None,
      profile: PathBuf::from(SYSTEM_PROFILE),
      output:  OutputFormat::Human,
      render:  RenderArgs::default(),
    });

    let cli = Cli::try_parse_from([
      "last",
      "--all",
      "--profile",
      "/my/profile",
      "--output",
      "json",
    ])
    .unwrap();
    assert_eq!(cli.command, Command::Last {
      count:   NonZeroUsize::MIN,
      all:     true,
      from:    None,
      to:      None,
      profile: PathBuf::from("/my/profile"),
      output:  OutputFormat::Json,
      render:  RenderArgs::default(),
    });
  }

  #[test]
  fn last_accepts_generation_ranges() {
    let cli =
      Cli::try_parse_from(["last", "--from", "300", "-t", "320"]).unwrap();
    assert!(matches!(cli.command, Command::Last {
      from: Some(300),
      to: Some(320),
      ..
    }));
  }

  #[test]
  fn last_rejects_invalid_arguments() {
    for args in [
      ["last", "0"].as_slice(),
      &["last", "-1"],
      &["last", "not-a-number"],
      &["last", "3", "--all"],
      &["last", "3", "--from", "2"],
      &["last", "--all", "--to", "2"],
      &["last", "--from", "-2"],
    ] {
      assert!(
        Cli::try_parse_from(args.iter().copied()).is_err(),
        "{args:?}"
      );
    }
  }

  #[test]
  fn snapshot_subcommand_parses() {
    let cli = Cli::try_parse_from(["snapshot", "/run/current-system"]).unwrap();
    assert_eq!(cli.command, Command::Snapshot {
      path: PathBuf::from("/run/current-system"),
    });
  }
}
