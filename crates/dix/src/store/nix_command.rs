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
  thread,
  time::Duration,
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
  store::StoreBackend,
};

#[derive(Debug)]
/// Uses nix commands to perform queries.
///
/// This is similar in implementation to the old `dix` in its early stages and
/// is supposed to be a final fallback if the direct queries on the database
/// fail. It is considerably slower than the direct queries and currently does
/// not support querying the whole dependency graph.
///
/// The internal internal command use is configurable but is expected
/// to be a drop-in replacement for the nix-store command.
pub struct CommandBackend {
  nix_store_cmd: String,
  nix_cmd:       String,
}

impl Display for CommandBackend {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "CommandBackend(nix='{cmd}', nix-store='{store}')",
      cmd = self.nix_cmd,
      store = self.nix_store_cmd
    )
  }
}

impl Default for CommandBackend {
  fn default() -> Self {
    Self {
      nix_store_cmd: "nix-store".to_owned(),
      nix_cmd:       "nix".to_owned(),
    }
  }
}

impl CommandBackend {
  #[must_use]
  pub const fn new(cmd_nix_store: String, cmd_nix: String) -> Self {
    Self {
      nix_store_cmd: cmd_nix_store,
      nix_cmd:       cmd_nix,
    }
  }
}

fn nix_command_query(cmd_store: &str, args: &[&str]) -> Result<Vec<StorePath>> {
  let command_str = format!("{cmd_store} {}", args.join(" "));
  tracing::debug!(command = %command_str, "executing nix command");
  let references = command_output(cmd_store, args);

  let query = references?;
  tracing::trace!(command = %command_str, "nix command executed successfully");
  if !query.status.success() {
    let stderr = String::from_utf8_lossy(&query.stderr);
    bail!(
      "nix command exited with non-zero status {status}: {err}",
      status = query.status,
      err = stderr.trim()
    );
  }
  // We just collect into a vec, as this method of
  // querying data is slow anyways
  let mut paths = Vec::new();
  for line in str::from_utf8(&query.stdout)?.lines() {
    let path = StorePath::try_from(PathBuf::from(line)).context(eyre!(
      "encountered invalid path in nix command output: {line}"
    ))?;
    paths.push(path);
  }

  Ok(paths)
}

fn command_output(cmd_store: &str, args: &[&str]) -> std::io::Result<Output> {
  const TEXT_FILE_BUSY: i32 = 26;

  for attempt in 0..3 {
    match Command::new(cmd_store).args(args).output() {
      Err(error) if error.raw_os_error() == Some(TEXT_FILE_BUSY) => {
        tracing::debug!(
          attempt = attempt + 1,
          command = cmd_store,
          "command executable is temporarily busy"
        );
        thread::sleep(Duration::from_millis(10));
      },
      result => return result,
    }
  }

  Command::new(cmd_store).args(args).output()
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

  fn query_closure_size(&self, path: &Path) -> Result<Size> {
    let sw_path = path.join("sw");
    let sw_path = sw_path.to_string_lossy();
    let cmd_res =
      command_output(&self.nix_cmd, &["path-info", "--closure-size", &sw_path])
        .wrap_err("Encountered error while executing nix command")?;

    if !cmd_res.status.success() {
      let stderr = String::from_utf8_lossy(&cmd_res.stderr);
      bail!(
        "nix command exited with non-zero status {status}: {err}",
        status = cmd_res.status,
        err = stderr.trim()
      );
    }
    let text = str::from_utf8(&cmd_res.stdout)?;

    if let Some(bytes_text) = text.split_whitespace().last()
      && let Ok(bytes) = bytes_text.parse::<u64>()
    {
      Ok(Size::from_bytes(bytes))
    } else {
      bail!("Unable to parse closure size from nix output")
    }
  }

  fn query_system_derivations(&self, system: &Path) -> Result<Vec<StorePath>> {
    nix_command_query(&self.nix_cmd, &[
      "--query",
      "--references",
      &*system.join("sw").to_string_lossy(),
    ])
  }

  fn query_dependents(&self, path: &Path) -> Result<Vec<StorePath>> {
    nix_command_query(&self.nix_cmd, &[
      "--query",
      "--requisites",
      &*path.to_string_lossy(),
    ])
  }
}

#[cfg(test)]
mod tests {
  use std::{
    io::Write as _,
    os::unix::fs::PermissionsExt,
  };

  use tempfile::TempDir;

  use super::*;

  const FAKE_PATHS: &str = r"/nix/store/0j3jwpcy0r9fk8ymmknq7d5bkjwg6kr3-gcc-15.2.0-lib
/nix/store/0j7cqjjjrx3dm875bpkwq8sqhc4c480f-sparklines-1.7-tex
/nix/store/0j7wz5lhxzzjnq637xyjv9arq6irplks-libdrm-2.4.129-bin
/nix/store/0j8ydh92l9hdjibg5d24nasxzha9ibvr-mbedtls-3.6.5
/nix/store/0j46vpswaga39ncjl4ck096pj2m096p7-jasper-4.2.8-lib
/nix/store/0j247ywaf8lvgrlqxrp0l4b2z6m8p2g4-libtpms-0.10.2
/nix/store/0jayna2g3yk787mwqaqx87zdxmpcm2n2-ghc-9.10.3_fish-completions
/nix/store/0jcl52a5dlf67mp74nnwdjv1wa1z92sp-python3.13-pycairo-1.28.0
/nix/store/0jf0xahxcckzxym71dl0vs2j0ifbzqld-initrd-linux-6.18.10
/nix/store/0jfk5fkidjnvx8zak8pf0z3xvzdp1kwy-libiff-0-unstable-2024-03-02
/nix/store/0jiignrsjnmzagq2mjd0mjqmjrm2r0rg-libXrandr-1.5.4
/nix/store/0jp30g5gnc38z94c9fs2irfb4lmnvwjd-fix-underspecified-getenv-prototype.patch
/nix/store/0jpfv9k5rf0xvj8h316358f7xpimagqr-qtwebengine-6.10.1
/nix/store/0jrc463plbk7r8gflnhwizydavlc86jy-perl-5.42.0-env
/nix/store/0jsv20rn7626s2k0mf8ri7y945aswr55-unzip-6.0
/nix/store/0jwivz9p3yz44q2ifz3ymjlx4hlgj9pr-libuv-1.50.0-dev
/nix/store/0jwkg5vcr4b4zi5r5xlg4nizd7h26776-libmpc-1.3.1
/nix/store/0jwmp61gj4gnl4xmynbcm7gsp14nr7jz-nspr-4.38.2
/nix/store/0jzmadwxrm1l561nf4ld36gw9ypjjxw4-maritime-1.0-tex
/nix/store/0k6bzcxjnwcym5ngav3hf0jfk1n1njpr-xintsession-0.4alpha-tex
/nix/store/0k9h6d8wqnn5fdjzzr9yvf239yhjj14x-blaze-textual-0.2.3.1-doc
/nix/store/0k22jqadc3f6nv17myymx0px6k4dsq8q-libXdamage-1.1.6
/nix/store/0kcbv1lw05nzjrvqckd833q6jrc6fzdn-williams-15878-tex
/nix/store/0kgwmi3n8ml2a041a5y9y9ycga3md4dq-pcre2-10.46
/nix/store/0khrcm7b4kq7mygx92hqj060ffxccvdh-fish_patched-completion-generator
/nix/store/0kp75kx6rpm8yxxzjbax8d2azv2gkv2b-Disable-methods-that-change-files-in-etc.patch
/nix/store/0kxq49jqjkcc1b429ik3p8jbzcwjc13a-ruby3.3-multi_json-1.15.0
/nix/store/0kzzha3wv08i1qbx1l7lxqwmvlz3968x-pypa-install-hook.sh
/nix/store/0l0hfrnp28kgkaxhyk073l6iqcwqq71p-dependent-map-0.4.0.1
/nix/store/0l5kp9nsmbvxyd5y289yf9jn21iqcydh-prettyref-3.0-tex
/nix/store/0l5rpjnxrrva2jmm31diafpk5kgic99l-onlyamsmath-0.20-tex
/nix/store/0l48ac6pnfzy8r76m93sj0jmdyr9i0gw-helvetic-61719-tex
/nix/store/0l489q22i2rhn30zprpcqrfm3q8js6cm-phfsvnwatermark-1.0-tex
/nix/store/0lid3kkldp5kcs9rxx88m9nnf5k5cnsn-no-sbom.patch
/nix/store/0lj6avd5ra4d4a76bzbcl350gw3ckrhg-ocgx2-0.60-tex
/nix/store/0lkrgp1y6gbvrx9yqkpwmlbg1j8yjyks-jujutsu-0.38.0
/nix/store/0lkwybfhk4hv6cmranqkal4dqpk3rhmn-cosmic-settings-1.0.5_fish-completions
/nix/store/0lln9m37mw7cjdf02krfpiwd4clwdlwl-python3.12-requests-2.32.3
/nix/store/0llv2ynbgmlbfhwbx6pl1jfrynx864mf-mobile-broadband-provider-info-20240407
/nix/store/0lpqn7fpwd2258kbya0ijip21mcs50vv-mbedtls-3.6.5
/nix/store/0lqxywpcv2v09l10231y38k641jd2ym3-utf8proc-2.11.2
/nix/store/0lwl0wy7888gfsvqwjf0xww19cz9n2vs-pst-poker-0.03b-tex
/nix/store/0lzpq0mfnl0rpix5fviakbzd5xjs0f8m-man-pages-6.16
/nix/store/0m1fhff5km9dyzfxf0a76spalr2y0znx-btop-1.4.6-fish-completions
/nix/store/0m1zhmxhlh1p5k5dqarvzz9lx52xdsv1-libfontenc-1.1.8
/nix/store/0m4i30rjfqhi2zlgfyra9v39fvmwywha-epigraph-keys-1.0-tex
/nix/store/0m5jvmyqqj8mxvczdj2rqcyq0fclkx7g-texlive-bin-big-2025-luahbtex
/nix/store/0m7l4qv84417cy2m8jlw7yxyn12ralyq-twolame-2017-09-27
/nix/store/0m8p1yj6k5fk7fpvj37krhbsnry8v70r-pmx-3.00-tex";

  const FAKE_CLOSURE_SIZE: i64 = 123_456_789;

  const FAKE_STORE_PATH: &str =
    "/nix/store/h9lc1dpi14z7is86ffhl3ld569138595-audit-tmpdir.sh";

  /// create a fake nix command that always
  /// outputs the same store paths given by `FAKE_PATHS`
  fn setup_fake_nix_command_mock_data() -> (TempDir, String) {
    let mock_command_content = format!(
      r#"#!/usr/bin/env sh
      for arg in "$@"; do
        if [ "$arg" = "--closure-size" ]; then
          echo "{closure_size}"
          exit 0
        fi
      done
    {echos}
    "#,
      closure_size = FAKE_CLOSURE_SIZE,
      echos = FAKE_PATHS
        .lines()
        .map(|path| format!("echo \"{path}\""))
        .collect::<Vec<String>>()
        .join("\n")
    );
    setup_fake_nix_command(&mock_command_content)
  }

  /// create a fake nix command that errors
  fn setup_fake_nix_command_error() -> (TempDir, String) {
    setup_fake_nix_command(
      r#"#!/usr/bin/env sh
      echo "I am not working correctly..."
      exit 1
    "#,
    )
  }

  /// The tempdir is returned as the actual directory
  /// is deleted once the value is dropped.
  fn setup_fake_nix_command(content: &str) -> (TempDir, String) {
    let cmd_dir = TempDir::new()
      .unwrap_or_else(|err| panic!("unable to create temp dir: {err}"));
    let mock_command = cmd_dir.path().join("mock-nix-store");
    {
      let mut file =
        std::fs::File::create(&mock_command).unwrap_or_else(|err| {
          panic!("unable to create mock command file: {err}")
        });
      file.write_all(content.as_bytes()).unwrap_or_else(|err| {
        panic!("unable to write mock command file: {err}")
      });
      file.sync_all().unwrap_or_else(|err| {
        panic!("unable to sync mock command file: {err}")
      });
    }
    std::fs::set_permissions(
      &mock_command,
      std::fs::Permissions::from_mode(0o500),
    )
    .unwrap_or_else(|err| panic!("unable to set permissions: {err}"));
    (cmd_dir, mock_command.to_string_lossy().to_string())
  }

  fn setup_fake_nix_command_backend() -> (TempDir, CommandBackend) {
    let (tmpdir, cmd) = setup_fake_nix_command_mock_data();
    (tmpdir, CommandBackend::new(cmd.clone(), cmd))
  }

  #[test]
  fn test_query_closure_size() {
    let (_tmpdir, backend) = setup_fake_nix_command_backend();
    let size = backend
      .query_closure_size(Path::new(FAKE_STORE_PATH))
      .unwrap();
    assert_eq!(size, Size::from_bytes(FAKE_CLOSURE_SIZE));
  }

  #[test]
  fn test_query_system_derivations() {
    let (_tmpdir, backend) = setup_fake_nix_command_backend();
    let mut references = backend
      .query_system_derivations(Path::new(FAKE_STORE_PATH))
      .unwrap();
    references.sort();
    let mut expected = FAKE_PATHS
      .lines()
      .map(|path| StorePath::try_from(PathBuf::from(path)).unwrap())
      .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(references, expected);
  }

  #[test]
  fn test_query_dependents() {
    let (_tmpdir, backend) = setup_fake_nix_command_backend();
    let mut references = backend
      .query_dependents(Path::new(FAKE_STORE_PATH))
      .unwrap();
    references.sort();
    let mut expected = FAKE_PATHS
      .lines()
      .map(|path| StorePath::try_from(PathBuf::from(path)).unwrap())
      .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(references, expected);
  }

  #[test]
  fn test_query_failing_command() {
    let (_tmpdir, cmd) = setup_fake_nix_command_error();
    let backend = CommandBackend::new(cmd.clone(), cmd);
    let result = backend.query_system_derivations(Path::new(FAKE_STORE_PATH));
    assert!(result.is_err());
  }

  #[test]
  fn test_nonexistent_nix_command() {
    let backend = CommandBackend::new(String::new(), String::new());
    let result = backend.query_system_derivations(Path::new(FAKE_STORE_PATH));
    assert!(result.is_err());
  }
  #[test]
  fn test_nonexistent_nix_store_command() {
    let backend = CommandBackend::new(String::new(), String::new());
    let result = backend.query_closure_size(Path::new(FAKE_STORE_PATH));
    assert!(result.is_err());
  }
}
