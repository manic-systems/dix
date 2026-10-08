use std::{
  fmt::{
    self,
    Display,
  },
  path::{
    Path,
    PathBuf,
  },
  process::Command,
};

use eyre::{
  Context,
  Result,
  bail,
  eyre,
};
use size::Size;

use crate::{
  StorePath,
  store::{
    StoreBackend,
    StorePathInfo,
  },
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

/// Runs a Nix command and returns its stdout.
fn run(command: &mut Command) -> Result<String> {
  let output = command
    .output()
    .wrap_err_with(|| format!("failed to execute {command:?}"))?;
  if !output.status.success() {
    bail!(
      "{command:?} exited with {status}: {err}",
      status = output.status,
      err = String::from_utf8_lossy(&output.stderr).trim()
    );
  }

  String::from_utf8(output.stdout)
    .wrap_err_with(|| format!("{command:?} printed invalid UTF-8"))
}

fn parse_store_path_output(output: &str) -> Result<Vec<StorePath>> {
  output
    .lines()
    .map(|line| {
      StorePath::try_from(PathBuf::from(line)).context(eyre!(
        "encountered invalid path in nix command output: {line}"
      ))
    })
    .collect()
}

fn parse_path_info_size_output(output: &str) -> Result<Vec<StorePathInfo>> {
  let mut infos = Vec::new();
  for line in output.lines() {
    let mut columns = line.split_whitespace();
    let path = columns
      .next()
      .ok_or_else(|| eyre!("missing path in nix path-info output line"))?;
    let bytes = columns
      .next()
      .ok_or_else(|| eyre!("missing NAR size in nix path-info output line"))?
      .parse::<i64>()
      .wrap_err("failed to parse NAR size from nix path-info output")?;

    infos.push(StorePathInfo::new(
      StorePath::try_from(PathBuf::from(path))?,
      Size::from_bytes(bytes),
    ));
  }

  Ok(infos)
}

impl StoreBackend for CommandBackend {
  /// Does nothing (we spawn a new process everytime).
  fn connect(&mut self) -> Result<()> {
    Ok(())
  }

  /// we don't really have a connection
  /// always returns true
  fn connected(&self) -> bool {
    true
  }

  /// there is nothing to close
  fn close(&mut self) -> Result<()> {
    Ok(())
  }

  fn query_system_derivations(&self, system: &Path) -> Result<Vec<StorePath>> {
    parse_store_path_output(&run(
      Command::new("nix-store")
        .args(["--query", "--references"])
        .arg(system.join("sw")),
    )?)
  }

  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>> {
    parse_store_path_output(&run(
      Command::new("nix-store")
        .args(["--query", "--requisites"])
        .arg(path),
    )?)
  }

  fn query_closure_path_info(&self, path: &Path) -> Result<Vec<StorePathInfo>> {
    parse_path_info_size_output(&run(
      Command::new("nix")
        .args(["path-info", "--recursive", "--size"])
        .arg(path),
    )?)
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
    let mut paths = parse_store_path_output(FAKE_PATHS).unwrap();
    paths.sort();
    let mut expected = FAKE_PATHS
      .lines()
      .map(|path| StorePath::try_from(PathBuf::from(path)).unwrap())
      .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(paths, expected);
  }

  #[test]
  fn parse_path_info_size_output_reads_nar_sizes() {
    let info = parse_path_info_size_output(&path_info_output()).unwrap();
    assert_eq!(info.len(), FAKE_PATHS.lines().count());
    assert_eq!(info[0].nar_size(), Size::from_bytes(1));
    assert_eq!(info[2].nar_size(), Size::from_bytes(3));
  }
}
