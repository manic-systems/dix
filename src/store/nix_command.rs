use std::{
  fmt::{
    self,
    Display,
  },
  process::Command,
};

use size::Size;

use crate::{
  Error,
  StorePath,
  StorePathInfo,
  store::StoreBackend,
};

/// Uses nix commands to perform queries.
///
/// This is similar in implementation to the old `dix` in its early stages and
/// is supposed to be a final fallback if the direct queries on the database
/// fail. It is considerably slower than the direct queries.
#[derive(Debug)]
pub struct CommandBackend;

impl Display for CommandBackend {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "CommandBackend")
  }
}

/// Renders `command` for error messages.
fn describe(command: &Command) -> String {
  format!("{command:?}")
}

/// Runs a Nix command.
///
/// # Returns
///
/// The command's standard output.
///
/// # Errors
///
/// Returns an error if the command cannot be run, exits unsuccessfully, or
/// prints invalid UTF-8.
fn run(command: &mut Command) -> Result<String, Error> {
  let output = command.output().map_err(|source| {
    Error::RunCommand {
      command: describe(command),
      source,
    }
  })?;
  if !output.status.success() {
    return Err(Error::CommandFailed {
      command: describe(command),
      status:  output.status,
      stderr:  String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    });
  }

  String::from_utf8(output.stdout).map_err(|source| {
    Error::CommandNotUtf8 {
      command: describe(command),
      source,
    }
  })
}

/// Parses Nix command output listing one store path per line.
///
/// # Returns
///
/// The store paths, in output order.
///
/// # Errors
///
/// Returns an error if a line is not a store path.
fn parse_store_path_output(
  command: &Command,
  output: &str,
) -> Result<Vec<StorePath>, Error> {
  output
    .lines()
    .map(|line| {
      StorePath::try_from(line.to_owned())
        .map_err(|_| unexpected_output(command, line))
    })
    .collect()
}

/// Reports `line` as unexpected output of `command`.
fn unexpected_output(command: &Command, line: &str) -> Error {
  Error::UnexpectedOutput {
    command: describe(command),
    line:    line.to_owned(),
  }
}

/// Parses `nix path-info --size` output, one path and NAR size per line.
///
/// # Returns
///
/// The store paths with their NAR sizes, in output order.
///
/// # Errors
///
/// Returns an error if a line lacks a column, or has an invalid path or size.
fn parse_path_info_size_output(
  command: &Command,
  output: &str,
) -> Result<Vec<StorePathInfo>, Error> {
  let mut infos = Vec::new();
  for line in output.lines() {
    let unexpected = || unexpected_output(command, line);
    let mut columns = line.split_whitespace();
    let path = columns.next().ok_or_else(unexpected)?;
    let bytes = columns
      .next()
      .and_then(|bytes| bytes.parse::<i64>().ok())
      .ok_or_else(unexpected)?;

    infos.push(StorePathInfo {
      path:     StorePath::try_from(path.to_owned())
        .map_err(|_| unexpected())?,
      nar_size: Size::from_bytes(bytes),
    });
  }

  Ok(infos)
}

/// Checks whether `path` is registered as valid in the Nix store.
///
/// # Errors
///
/// Returns an error if `nix-store --check-validity` fails.
fn is_valid(path: &StorePath) -> Result<bool, Error> {
  // With `--print-invalid`, invalid paths are printed instead of failing.
  let invalid = run(
    Command::new("nix-store")
      .args(["--check-validity", "--print-invalid"])
      .arg(path.as_os_str()),
  )?;
  Ok(invalid.trim().is_empty())
}

impl StoreBackend for CommandBackend {
  type Error = Error;

  fn query_system_derivations(
    &self,
    system: &StorePath,
  ) -> Result<Vec<StorePath>, Error> {
    let mut command = Command::new("nix-store");
    command
      .args(["--query", "--references"])
      .arg(system.join("sw"));
    let output = run(&mut command)?;
    parse_store_path_output(&command, &output)
  }

  fn query_closure_path_info(
    &self,
    path: &StorePath,
  ) -> Result<Vec<StorePathInfo>, Error> {
    let mut command = Command::new("nix");
    command
      .args(["path-info", "--recursive", "--size"])
      .arg(path.as_os_str());
    match run(&mut command) {
      Ok(output) => parse_path_info_size_output(&command, &output),
      // An empty closure tells the caller the path is not valid, as the
      // database backend does. If validity cannot be checked either, the
      // original failure is the more useful error.
      Err(error @ Error::CommandFailed { .. }) => {
        match is_valid(path) {
          Ok(false) => Ok(Vec::new()),
          Ok(true) => Err(error),
          Err(validity_error) => {
            tracing::debug!(
              validity_error = super::error_chain(&validity_error),
              "failed to check path validity"
            );
            Err(error)
          },
        }
      },
      Err(error) => Err(error),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const FAKE_PATHS: &str = "\
/nix/store/0j3jwpcy0r9fk8ymmknq7d5bkjwg6kr3-gcc-15.2.0-lib
/nix/store/0j7cqjjjrx3dm875bpkwq8sqhc4c480f-sparklines-1.7-tex
/nix/store/0j8ydh92l9hdjibg5d24nasxzha9ibvr-mbedtls-3.6.5";

  fn path_info_output() -> String {
    FAKE_PATHS
      .lines()
      .enumerate()
      .map(|(index, path)| format!("{path} {}", index + 1))
      .collect::<Vec<String>>()
      .join("\n")
  }

  #[test]
  fn parse_store_path_output_reads_paths() {
    let mut paths =
      parse_store_path_output(&Command::new("nix-store"), FAKE_PATHS).unwrap();
    paths.sort();
    let mut expected = FAKE_PATHS
      .lines()
      .map(|path| StorePath::try_from(path.to_owned()).unwrap())
      .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(paths, expected);
  }

  #[test]
  fn parse_path_info_size_output_reads_nar_sizes() {
    let info =
      parse_path_info_size_output(&Command::new("nix"), &path_info_output())
        .unwrap();
    assert_eq!(info.len(), FAKE_PATHS.lines().count());
    assert_eq!(info[0].nar_size, Size::from_bytes(1));
    assert_eq!(info[2].nar_size, Size::from_bytes(3));
  }
}
