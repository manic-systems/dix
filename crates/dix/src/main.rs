use std::{
  env,
  fmt::{
    self,
    Write as _,
  },
  fs,
  io::{
    self,
    IsTerminal as _,
    Write as _,
  },
  num::NonZeroUsize,
  path::{
    Path,
    PathBuf,
  },
};

use clap::Parser as _;
use dix::{
  DiffReport,
  RenderOptions,
  json,
};
use eyre::eyre;
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

#[derive(clap::Parser, Debug)]
#[command(version, about)]
struct Cli {
  #[command(subcommand)]
  command: Command,

  #[command(flatten)]
  verbose: clap_verbosity_flag::Verbosity,

  /// Controls when to use color.
  #[arg(
      long,
      default_value_t = clap::ColorChoice::Auto,
      value_name = "WHEN",
      global = true,
  )]
  color: clap::ColorChoice,
}

#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum Command {
  /// Show the differences between two store paths.
  Diff {
    old_path: PathBuf,
    new_path: PathBuf,

    /// Select the output format to use.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    output: OutputFormat,

    #[command(flatten)]
    render: RenderArgs,
  },
  /// Show successive changes ending at the current profile generation.
  Last {
    /// Number of transitions to show.
    #[arg(default_value_t = NonZeroUsize::MIN)]
    count: NonZeroUsize,

    /// Show every adjacent pair of existing generations in numerical order.
    #[arg(long, conflicts_with_all = ["count", "from", "to"])]
    all: bool,

    /// Show generations numbered from GENERATION onwards.
    #[arg(short, long, value_name = "GENERATION", conflicts_with = "count")]
    from: Option<u64>,

    /// Show generations numbered up to GENERATION.
    #[arg(short, long, value_name = "GENERATION", conflicts_with = "count")]
    to: Option<u64>,

    /// Profile link to inspect.
    #[arg(long, default_value = SYSTEM_PROFILE)]
    profile: PathBuf,

    /// Select the output format to use.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    output: OutputFormat,

    #[command(flatten)]
    render: RenderArgs,
  },
  /// Print the closure of a store path as a versioned JSON snapshot.
  Snapshot { path: PathBuf },
}

/// Options for the human-readable output.
#[derive(clap::Args, Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RenderArgs {
  /// Show packages that only changed in amount or size instead of
  /// summarizing them.
  #[arg(long)]
  full: bool,
}

impl From<RenderArgs> for RenderOptions {
  fn from(args: RenderArgs) -> Self {
    Self { full: args.full }
  }
}

/// Determines the output format to be used by dix.
#[derive(Debug, Clone, Copy, clap::ValueEnum, Eq, PartialEq)]
enum OutputFormat {
  /// Output in the default dix format highlighting version changes.
  Human,
  /// Display the output as JSON for machine parsing.
  Json,
}

fn main() -> eyre::Result<()> {
  let Cli {
    command,
    verbose,
    color,
  } = Cli::parse();

  yansi::whenever(match color {
    clap::ColorChoice::Auto => yansi::Condition::from(should_style),
    clap::ColorChoice::Always => yansi::Condition::ALWAYS,
    clap::ColorChoice::Never => yansi::Condition::NEVER,
  });

  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::builder()
        .with_default_directive(match verbose.log_level_filter() {
          clap_verbosity_flag::log::LevelFilter::Off
          | clap_verbosity_flag::log::LevelFilter::Error => {
            tracing::Level::ERROR.into()
          },
          clap_verbosity_flag::log::LevelFilter::Warn => {
            tracing::Level::WARN.into()
          },
          clap_verbosity_flag::log::LevelFilter::Info => {
            tracing::Level::INFO.into()
          },
          clap_verbosity_flag::log::LevelFilter::Debug => {
            tracing::Level::DEBUG.into()
          },
          clap_verbosity_flag::log::LevelFilter::Trace => {
            tracing::Level::TRACE.into()
          },
        })
        .from_env_lossy(),
    )
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
fn print_snapshot(path: &Path) -> eyre::Result<()> {
  let document = dix::query_snapshot_document(path)?;
  let mut out = io::stdout().lock();
  serde_json::to_writer(&mut out, &document)?;
  writeln!(out)?;
  Ok(())
}

/// Prints the diff between `old_path` and `new_path` to stdout.
///
/// # Returns
///
/// `Ok(())` once the diff is printed.
///
/// # Errors
///
/// Returns an error if a path does not exist, the store cannot be queried, or
/// the output cannot be written.
fn print_diff(
  old_path: &Path,
  new_path: &Path,
  output: OutputFormat,
  options: RenderOptions,
) -> eyre::Result<()> {
  for (name, path) in [("old", old_path), ("new", new_path)] {
    if !path.exists() {
      return Err(eyre!(
        "{name} profile path does not exist: {}",
        path.display()
      ));
    }
  }

  tracing::info!(old_path = %old_path.display(), new_path = %new_path.display(), "paths validated");

  let report = dix::query_diff_report(old_path, new_path)?;
  match output {
    OutputFormat::Human => {
      let hidden = display_diff(old_path, new_path, &report, options)?;
      print_hidden_note(hidden)
    },
    OutputFormat::Json => json::write_report(io::stdout(), &report),
  }
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
) -> eyre::Result<()> {
  let generations = select_generations(profile, span)?;
  let paths = generations
    .iter()
    .map(|generation| generation.path.as_path())
    .collect::<Vec<_>>();
  let reports = dix::query_adjacent_diff_reports(&paths)?;
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
      print_hidden_note(hidden)
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
) -> eyre::Result<usize> {
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

/// Prints a note about the `hidden` package diffs to stdout, if there are any.
///
/// # Returns
///
/// `Ok(())` once the note is printed.
///
/// # Errors
///
/// Returns an error if the output cannot be written.
fn print_hidden_note(hidden: usize) -> eyre::Result<()> {
  if hidden > 0 {
    writeln!(
      WriteFmt(io::stdout()),
      "\n{header}: {hidden} hidden {hint}",
      header = "NOTE".bold(),
      hidden = hidden.yellow(),
      hint = "(--full to show)".dim(),
    )?;
  }
  Ok(())
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
    let cli = Cli::try_parse_from(["dix", "diff", "/old", "/new"]).unwrap();
    assert_eq!(cli.command, Command::Diff {
      old_path: PathBuf::from("/old"),
      new_path: PathBuf::from("/new"),
      output:   OutputFormat::Human,
      render:   RenderArgs::default(),
    });
  }

  #[test]
  fn bare_paths_are_rejected() {
    assert!(Cli::try_parse_from(["dix", "/old", "/new"]).is_err());
  }

  #[test]
  fn last_subcommand_parses() {
    let cli = Cli::try_parse_from(["dix", "last", "3"]).unwrap();
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
      "dix",
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
      Cli::try_parse_from(["dix", "last", "--from", "300", "-t", "320"])
        .unwrap();
    assert!(matches!(cli.command, Command::Last {
      from: Some(300),
      to: Some(320),
      ..
    }));
  }

  #[test]
  fn last_rejects_invalid_arguments() {
    for args in [
      ["dix", "last", "0"].as_slice(),
      &["dix", "last", "-1"],
      &["dix", "last", "not-a-number"],
      &["dix", "last", "3", "--all"],
      &["dix", "last", "3", "--from", "2"],
      &["dix", "last", "--all", "--to", "2"],
      &["dix", "last", "--from", "-2"],
    ] {
      assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
    }
  }

  #[test]
  fn snapshot_subcommand_parses() {
    let cli =
      Cli::try_parse_from(["dix", "snapshot", "/run/current-system"]).unwrap();
    assert_eq!(cli.command, Command::Snapshot {
      path: PathBuf::from("/run/current-system"),
    });
  }
}
