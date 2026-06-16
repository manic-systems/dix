use std::{
  fmt::{
    self,
    Display,
  },
  path::{
    Path,
    PathBuf,
  },
  process::{
    Command,
    Output,
  },
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
///
/// The internal command use is configurable but is expected to be a drop-in
/// replacement for the nix-store command.
#[derive(Debug)]
pub struct CommandBackend {
  nix_store_cmd: String,
  nix_cmd:       String,
  store_url:     Option<String>,
}

impl Display for CommandBackend {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "CommandBackend(nix='{cmd}', nix-store='{store}'",
      cmd = self.nix_cmd,
      store = self.nix_store_cmd,
    )?;
    if let Some(store_url) = &self.store_url {
      write!(f, ", store='{store_url}'")?;
    }
    write!(f, ")")
  }
}

impl Default for CommandBackend {
  fn default() -> Self {
    Self {
      nix_store_cmd: "nix-store".to_owned(),
      nix_cmd:       "nix".to_owned(),
      store_url:     None,
    }
  }
}

impl CommandBackend {
  #[must_use]
  pub const fn new(cmd_nix_store: String, cmd_nix: String) -> Self {
    Self {
      nix_store_cmd: cmd_nix_store,
      nix_cmd:       cmd_nix,
      store_url:     None,
    }
  }

  /// Use a specific Nix store URI for command-backed queries.
  #[must_use]
  pub fn store_url(mut self, store_url: impl Into<String>) -> Self {
    self.store_url = Some(store_url.into());
    self
  }

  fn nix_store_command(&self) -> Command {
    let mut command = Command::new(&self.nix_store_cmd);
    if let Some(store_url) = &self.store_url {
      command.arg("--store").arg(store_url);
    }
    command
  }

  fn nix_command(&self, subcommand: &str) -> Command {
    let mut command = Command::new(&self.nix_cmd);
    command.arg(subcommand);
    if let Some(store_url) = &self.store_url {
      command.arg("--store").arg(store_url);
    }
    command
  }
}

fn parse_store_path_output(output: &Output) -> Result<Vec<StorePath>> {
  str::from_utf8(&output.stdout)?
    .lines()
    .map(|line| {
      StorePath::try_from(PathBuf::from(line)).context(eyre!(
        "encountered invalid path in nix command output: {line}"
      ))
    })
    .collect()
}

fn parse_path_info_size_output(output: &Output) -> Result<Vec<StorePathInfo>> {
  let text = str::from_utf8(&output.stdout)?;
  let mut infos = Vec::new();
  for line in text.lines() {
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
    let output = self
      .nix_store_command()
      .args(["--query", "--references"])
      .arg(system.join("sw"))
      .output()
      .wrap_err("Encountered error while executing nix-store command")?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      bail!(
        "nix-store command exited with non-zero status {status}: {err}",
        status = output.status,
        err = stderr.trim()
      );
    }

    parse_store_path_output(&output)
  }

  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>> {
    let output = self
      .nix_store_command()
      .args(["--query", "--requisites"])
      .arg(path)
      .output()
      .wrap_err("Encountered error while executing nix-store command")?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      bail!(
        "nix-store command exited with non-zero status {status}: {err}",
        status = output.status,
        err = stderr.trim()
      );
    }

    parse_store_path_output(&output)
  }

  fn query_closure_path_info(&self, path: &Path) -> Result<Vec<StorePathInfo>> {
    let output = self
      .nix_command("path-info")
      .args(["--recursive", "--size"])
      .arg(path)
      .output()
      .wrap_err("Encountered error while executing nix command")?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      bail!(
        "nix command exited with non-zero status {status}: {err}",
        status = output.status,
        err = stderr.trim()
      );
    }

    parse_path_info_size_output(&output)
  }
}

#[cfg(test)]
mod tests {
  use std::os::unix::process::ExitStatusExt;

  use super::*;

  const FAKE_PATHS: &str = "\
/nix/store/0j3jwpcy0r9fk8ymmknq7d5bkjwg6kr3-gcc-15.2.0-lib
/nix/store/0j7cqjjjrx3dm875bpkwq8sqhc4c480f-sparklines-1.7-tex
/nix/store/0j8ydh92l9hdjibg5d24nasxzha9ibvr-mbedtls-3.6.5";

  fn mock_output(stdout: impl Into<Vec<u8>>) -> Output {
    Output {
      status: ExitStatusExt::from_raw(0),
      stdout: stdout.into(),
      stderr: Vec::new(),
    }
  }

  fn path_info_output() -> String {
    FAKE_PATHS
      .lines()
      .enumerate()
      .map(|(index, path)| format!("{path} {}", index + 1))
      .collect::<Vec<String>>()
      .join("\n")
  }

  fn command_args(command: &Command) -> Vec<String> {
    command
      .get_args()
      .map(|arg| arg.to_string_lossy().into_owned())
      .collect()
  }

  #[test]
  fn store_url_is_added_to_nix_store_commands() {
    let backend = CommandBackend::default().store_url("ssh://builder");
    let command = backend.nix_store_command();

    assert_eq!(command_args(&command), vec!["--store", "ssh://builder"]);
  }

  #[test]
  fn parse_store_path_output_reads_paths() {
    let mut paths = parse_store_path_output(&mock_output(FAKE_PATHS)).unwrap();
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
    let info =
      parse_path_info_size_output(&mock_output(path_info_output())).unwrap();
    assert_eq!(info.len(), FAKE_PATHS.lines().count());
    assert_eq!(info[0].nar_size(), Size::from_bytes(1));
    assert_eq!(info[2].nar_size(), Size::from_bytes(3));
  }
}
