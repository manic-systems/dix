# Diff Nix

A blazingly fast tool to diff Nix related things.

Currently only supports closures (a derivation graph, such as a system build or
package).

![output of `dix /nix/var/nix/profiles/system-69-link/ /run/current-system`](.github/dix.png)

## Usage
```bash
$ dix --help
Diff Nix

Usage: dix [OPTIONS] <OLD_PATH> <NEW_PATH>

Arguments:
  <OLD_PATH>


  <NEW_PATH>


Options:
  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

      --color <WHEN>
          Controls when to use color

          [default: auto]
          [possible values: auto, always, never]

      --force-correctness
          Fall back to a backend chain that skips SQLite immutable mode.

          This is relevant if the output of dix is to be used for more critical applications and not just as human-readable overview.

          The default backend falls back to opening Nix's SQLite database with `?immutable=1` if the normal connection fails. That is faster than Nix commands, but can be inaccurate if the database is being written to at the same time.

      --output <OUTPUT>
          Select the output format to use

          Possible values:
          - human: Output in the default dix format highlighting version changes
          - json:  Display the output as JSON for machine parsing (requires `json` feature)

          [default: human]

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

$ dix /nix/var/profiles/system-69-link /run/current-system
```

# Usage in CI

If you're planning on using dix in CI, you might want to set the
`--force-correctness` flag to ensure that the results are definitely accurate.\
Dix will fall back to a connection using `?immutable=1` to Nix's SQLite database
if it fails connecting normally; This can however result in inaccurate output if
the database is being written to at the same time.\
Passing `--force-correctness` will make dix fall back to Nix commands if
connection to the database fails, which ensures correct output, potentially at
the cost of speed.

## Releasing

`dix-diff` is a separate crate because it owns the pure package/version diff
engine. Publish it before publishing `dix`; the `dix` package depends on the
same exact `dix-diff` version.

```sh
cargo publish -p dix-diff
cargo publish -p dix
```

## Contributing

If you have any problems, feature requests or want to contribute code or want to
provide input in some other way, feel free to create an issue or a pull request!

## Thanks

Huge thanks to [nvd](https://git.sr.ht/~khumba/nvd) for the original idea! Dix
is heavily inspired by this and basically just a "Rewrite it in Rust" version of
nvd, with a few things like version diffing done better.

Furthermore, many thanks to the amazing people who made this projects possible
by contributing code and offering advice:

- [@Dragyx](https://github.com/Dragyx) - Cool SQL queries. Much of dix's speed
  is thanks to him.
- [@NotAShelf](https://github.com/NotAShelf) - Implementing proper error
  handling.
- [@RGBCube](https://github.com/RGBCube) - Giving the codebase a deep scrub.

## License

Dix is licensed under [GPLv3](LICENSE.md). See the license file for more
details.
