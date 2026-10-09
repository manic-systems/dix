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
  process::ExitCode,
  thread,
  time::Duration,
};

use clap::Parser as _;
use dix::{
  DiffReport,
  RenderOptions,
  json,
};
use eyre::{
  WrapErr as _,
  eyre,
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

/// Describes the exit status, shown after the help of commands that compare.
const EXIT_STATUS_HELP: &str =
  "Exit status is 0 if the closures are the same, 1 if different, 2 if \
   trouble. Without --exit-code or --quiet, differences also exit with 0.";

#[derive(clap::Parser, Debug)]
#[command(version, about, after_help = EXIT_STATUS_HELP)]
struct Cli {
  #[command(subcommand)]
  command: Command,

  /// Increase logging verbosity.
  #[arg(
    short,
    long,
    action = clap::ArgAction::Count,
    global = true,
    display_order = 100,
  )]
  verbose: u8,

  /// Controls when to use color.
  #[arg(
      long,
      default_value_t = clap::ColorChoice::Auto,
      value_name = "WHEN",
      global = true,
      display_order = 100,
  )]
  color: clap::ColorChoice,
}

#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum Command {
  /// Show the differences between two store paths.
  #[command(after_help = EXIT_STATUS_HELP)]
  Diff {
    old_path: PathBuf,
    new_path: PathBuf,

    /// Select the output format to use.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    output: OutputFormat,

    #[command(flatten)]
    render: RenderArgs,

    #[command(flatten)]
    status: StatusArgs,
  },
  /// Show successive changes ending at the current profile generation.
  #[command(after_help = EXIT_STATUS_HELP)]
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

    #[command(flatten)]
    status: StatusArgs,
  },
  /// Print the closure of a store path as a versioned JSON snapshot.
  Snapshot { path: PathBuf },
}

/// Options for the human-readable output.
#[derive(clap::Args, Debug, Clone, Copy, PartialEq, Eq)]
struct RenderArgs {
  /// Also show packages whose version did not change.
  #[arg(long)]
  full: bool,

  /// Hide packages whose version did not change if their size changed by
  /// less than SIZE (e.g. 4096, 512KiB, 10MB).
  #[arg(
    long,
    value_name = "SIZE",
    default_value_t = RenderOptions::default().min_size_delta,
    value_parser = parse_size,
  )]
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

/// Options for signalling differences through the exit status.
#[derive(clap::Args, Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StatusArgs {
  /// Exit with 1 if the compared closures differ, and 0 if they do not.
  #[arg(long)]
  exit_code: bool,

  /// Print nothing. Implies --exit-code.
  #[arg(short, long)]
  quiet: bool,
}

impl StatusArgs {
  /// Selects the format to print the comparison in, or `None` to print
  /// nothing.
  fn output(self, format: OutputFormat) -> Option<OutputFormat> {
    (!self.quiet).then_some(format)
  }

  /// Maps whether the compared closures `changed` to the exit status.
  const fn exit_status(self, changed: bool) -> ExitCode {
    if changed && (self.exit_code || self.quiet) {
      ExitCode::FAILURE
    } else {
      ExitCode::SUCCESS
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
#[derive(Debug, Clone, Copy, clap::ValueEnum, Eq, PartialEq)]
enum OutputFormat {
  /// Output in the default dix format highlighting version changes.
  Human,
  /// Display the output as JSON for machine parsing.
  Json,
}

fn main() -> ExitCode {
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
        .with_default_directive(
          match verbose {
            0 => tracing::Level::ERROR,
            1 => tracing::Level::WARN,
            2 => tracing::Level::INFO,
            3 => tracing::Level::DEBUG,
            _ => tracing::Level::TRACE,
          }
          .into(),
        )
        .from_env_lossy(),
    )
    .with_writer(io::stderr)
    .with_ansi(should_style())
    .with_target(false)
    .without_time()
    .init();

  let result = match command {
    Command::Diff {
      old_path,
      new_path,
      output,
      render,
      status,
    } => {
      print_diff(&old_path, &new_path, status.output(output), render.into())
        .map(|changed| status.exit_status(changed))
    },
    Command::Last {
      count,
      all,
      from,
      to,
      profile,
      output,
      render,
      status,
    } => {
      let span = if all || from.is_some() || to.is_some() {
        Span::Range { from, to }
      } else {
        Span::Latest(count)
      };
      print_last(&profile, span, status.output(output), render.into())
        .map(|changed| status.exit_status(changed))
    },
    Command::Snapshot { path } => {
      print_snapshot(&path).map(|()| ExitCode::SUCCESS)
    },
  };

  // std flushes stdout once `main` returns, but ignores errors by then. Output
  // without a trailing newline, like JSON, may still be buffered here.
  let result = result.and_then(|status| {
    io::stdout().flush()?;
    Ok(status)
  });

  result.unwrap_or_else(|error| {
    let _ = writeln!(io::stderr(), "Error: {error:?}");
    ExitCode::from(2)
  })
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

/// Prints the diff between `old_path` and `new_path` to stdout in `output`, or
/// nothing if `output` is `None`.
///
/// # Returns
///
/// Whether the closures of `old_path` and `new_path` differ.
///
/// # Errors
///
/// Returns an error if a path does not exist or cannot be accessed, the store
/// cannot be queried, or the output cannot be written.
fn print_diff(
  old_path: &Path,
  new_path: &Path,
  output: Option<OutputFormat>,
  options: RenderOptions,
) -> eyre::Result<bool> {
  for (name, path) in [("old", old_path), ("new", new_path)] {
    let exists = path.try_exists().wrap_err_with(|| {
      format!("failed to access {name} profile path: {}", path.display())
    })?;
    if !exists {
      return Err(eyre!(
        "{name} profile path does not exist: {}",
        path.display()
      ));
    }
  }

  tracing::info!(old_path = %old_path.display(), new_path = %new_path.display(), "paths validated");

  let report = dix::query_diff_report(old_path, new_path)?;
  match output {
    None => {},
    Some(OutputFormat::Human) => {
      let hidden = display_diff(old_path, new_path, &report, options)?;
      print_hidden_note(hidden)?;
    },
    Some(OutputFormat::Json) => json::write_report(io::stdout(), &report)?,
  }
  Ok(report.has_changes())
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
/// selected by `span` to stdout in `output`, or nothing if `output` is `None`.
///
/// # Returns
///
/// Whether the closures of any transition differ.
///
/// # Errors
///
/// Returns an error if the generations cannot be selected, the store cannot
/// be queried, or the output cannot be written.
fn print_last(
  profile: &Path,
  span: Span,
  output: Option<OutputFormat>,
  options: RenderOptions,
) -> eyre::Result<bool> {
  let generations = select_generations(profile, span)?;
  let paths = generations
    .iter()
    .map(|generation| generation.path.as_path())
    .collect::<Vec<_>>();
  let reports = dix::query_adjacent_diff_reports(&paths)?;
  let changed = reports.iter().any(DiffReport::has_changes);
  let transitions = generations.array_windows().zip(&reports);

  match output {
    None => {},
    Some(OutputFormat::Human) => {
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
      print_hidden_note(hidden)?;
    },
    Some(OutputFormat::Json) => {
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
      serde_json::to_writer(io::stdout(), &transitions)?;
    },
  }
  Ok(changed)
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
  if hidden == 0 {
    return Ok(());
  }

  let note = |count: &dyn fmt::Display| {
    format!(
      "{header}: {count} hidden {hint}",
      header = "NOTE".bold(),
      count = count.yellow(),
      hint = "(--full to show)".dim(),
    )
  };
  let mut out = io::stdout().lock();
  writeln!(out)?;

  // Six seven.
  if hidden == 67 && yansi::is_enabled() && out.is_terminal() {
    for frame in ["⁶₇", "₆⁷"].iter().cycle().take(8) {
      write!(out, "\r{}", note(frame))?;
      out.flush()?;
      thread::sleep(Duration::from_millis(300));
    }
    write!(out, "\r")?;
  }

  writeln!(out, "{}", note(&hidden))?;
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
      status:   StatusArgs::default(),
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
      status:  StatusArgs::default(),
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
      status:  StatusArgs::default(),
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
