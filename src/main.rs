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
  },
  path::{
    Path,
    PathBuf,
  },
};

use clap::Parser as _;
#[cfg(feature = "json")] use dix::json;
use eyre::eyre;
use yansi::Paint as _;

// Moar speed! Shaves about a millisecond off.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct WriteFmt<W: io::Write>(W);

impl<W: io::Write> fmt::Write for WriteFmt<W> {
  fn write_str(&mut self, string: &str) -> fmt::Result {
    self.0.write_all(string.as_bytes()).map_err(|_| fmt::Error)
  }
}

#[derive(clap::Parser, Debug)]
#[command(version, about)]
struct Cli {
  old_path: PathBuf,
  new_path: PathBuf,

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

  /// Fall back to a backend chain that skips `SQLite` immutable mode.
  ///
  /// This is relevant if the output of dix is to be used for more
  /// critical applications and not just as human-readable overview.
  ///
  /// The default backend falls back to opening Nix's `SQLite` database with
  /// `?immutable=1` if the normal connection fails. That is faster than Nix
  /// commands, but can be inaccurate if the database is being written to at
  /// the same time.
  #[arg(long, default_value_t = false, global = true)]
  force_correctness: bool,

  /// Select the output format to use.
  #[arg(long, value_enum, default_value_t = OutputFormat::Human, global = true)]
  output: OutputFormat,
}

/// Determines the output format to be used by dix.
#[derive(Debug, Clone, clap::ValueEnum, Eq, PartialEq)]
enum OutputFormat {
  /// Output in the default dix format highlighting version changes.
  Human,
  /// Display the output as JSON for machine parsing (requires `json` feature).
  Json,
}

fn main() -> eyre::Result<()> {
  let Cli {
    old_path,
    new_path,
    verbose,
    color,
    force_correctness,
    output,
  } = Cli::parse();

  tracing::debug!(
    old_path = %old_path.display(),
    new_path = %new_path.display(),
    force_correctness = force_correctness,
    "starting dix"
  );

  // Validate that both paths exist before proceeding
  if !old_path.exists() {
    tracing::error!(path = %old_path.display(), "old profile path does not exist");
    return Err(eyre!(
      "old profile path does not exist: {}",
      old_path.display()
    ));
  }
  if !new_path.exists() {
    tracing::error!(path = %new_path.display(), "new profile path does not exist");
    return Err(eyre!(
      "new profile path does not exist: {}",
      new_path.display()
    ));
  }

  tracing::info!(old_path = %old_path.display(), new_path = %new_path.display(), "paths validated");

  let style_enabled = match color {
    clap::ColorChoice::Auto => should_style(),
    clap::ColorChoice::Always => true,
    clap::ColorChoice::Never => false,
  };
  yansi::whenever(if style_enabled {
    yansi::Condition::ALWAYS
  } else {
    yansi::Condition::NEVER
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
    .with_ansi(style_enabled)
    .with_target(false)
    .without_time()
    .init();

  if force_correctness {
    tracing::warn!(
      "Falling back to slower but more robust backends (force_correctness is \
       set)."
    );
  }
  match output {
    OutputFormat::Human => {
      display_diff(&old_path, &new_path, force_correctness)?;
    },
    #[cfg(feature = "json")]
    OutputFormat::Json => {
      json::display_diff(&old_path, &new_path, force_correctness)?;
    },
    #[cfg(not(feature = "json"))]
    OutputFormat::Json => {
      return Err(eyre!(
        "The 'json' feature is required to use '--json-output'."
      ));
    },
  }

  Ok(())
}

fn display_diff(
  old_path: &Path,
  new_path: &Path,
  force_correctness: bool,
) -> eyre::Result<()> {
  let mut out = WriteFmt(io::stdout());

  tracing::info!("starting diff computation");

  writeln!(
    out,
    "{arrows} {old}",
    arrows = "<<<".bold(),
    old = old_path.display(),
  )?;
  writeln!(
    out,
    "{arrows} {new}",
    arrows = ">>>".bold(),
    new = fs::canonicalize(new_path)
      .unwrap_or_else(|_| new_path.to_path_buf())
      .display(),
  )?;

  let report = dix::query_diff_report(old_path, new_path, force_correctness)?;
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
