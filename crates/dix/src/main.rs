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
  path::{
    Path,
    PathBuf,
  },
};

use clap::Parser as _;
use dix::json;
use eyre::eyre;
use yansi::Paint as _;

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
  },
  /// Print the closure of a store path as a versioned JSON snapshot.
  Snapshot { path: PathBuf },
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
    } => print_diff(&old_path, &new_path, output),
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

  match output {
    OutputFormat::Human => display_diff(old_path, new_path),
    OutputFormat::Json => json::display_diff(old_path, new_path),
  }
}

/// Renders the diff between `old_path` and `new_path` for humans to stdout.
///
/// # Returns
///
/// `Ok(())` once the diff is rendered.
///
/// # Errors
///
/// Returns an error if the store cannot be queried or the output cannot be
/// written.
fn display_diff(old_path: &Path, new_path: &Path) -> eyre::Result<()> {
  let mut out = WriteFmt(io::stdout());

  tracing::info!("starting diff computation");

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

  let report = dix::query_diff_report(old_path, new_path)?;
  dix::write_diff_report(&mut out, &report)?;

  tracing::info!("diff computation complete");

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
    });
  }

  #[test]
  fn bare_paths_are_rejected() {
    assert!(Cli::try_parse_from(["dix", "/old", "/new"]).is_err());
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
