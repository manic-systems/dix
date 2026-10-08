# Diff Nix

A blazingly fast tool to diff Nix related things.

Currently only supports closures (a derivation graph, such as a system build or
package).

![output of `dix diff /nix/var/nix/profiles/system-69-link/ /run/current-system`](.github/dix.png)

## Usage
```bash
$ dix --help
Diff Nix

Usage: dix [OPTIONS] <COMMAND>

Commands:
  diff      Show the differences between two store paths
  snapshot  Print the closure of a store path as a versioned JSON snapshot
  help      Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...    Increase logging verbosity
  -q, --quiet...      Decrease logging verbosity
      --color <WHEN>  Controls when to use color [default: auto] [possible values: auto, always, never]
  -h, --help          Print help
  -V, --version       Print version

$ dix diff /nix/var/profiles/system-69-link /run/current-system
```

# Snapshots

`dix snapshot <PATH>` prints the closure of a store path as JSON, so it can be
diffed on another machine. [nh](https://github.com/nix-community/nh) uses this
to diff deployments to remote hosts, which requires dix on the target host.

```json
{
  "version": 1,
  "path": "/nix/store/<hash>-nixos-system-...",
  "closure": [{ "path": "/nix/store/<hash>-bash-5.2.15", "narSize": 5000000 }],
  "selected": ["/nix/store/<hash>-bash-5.2.15"]
}
```

- `version` is the format version. It is bumped on incompatible changes.
- `path` is the canonical store path, with symlinks resolved.
- `closure` lists every path in the closure with its NAR size in bytes.
- `selected` lists the packages selected by a NixOS system's `system-path`.

Snapshots always use the correctness-focused store backends.

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
