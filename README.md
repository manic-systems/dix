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
  last      Show successive changes ending at the current profile generation
  snapshot  Print the closure of a store path as a versioned JSON snapshot
  help      Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...    Increase logging verbosity
  -q, --quiet...      Decrease logging verbosity
      --color <WHEN>  Controls when to use color [default: auto] [possible values: auto, always, never]
  -h, --help          Print help
  -V, --version       Print version

$ dix diff /nix/var/profiles/system-69-link /run/current-system
$ dix last
$ dix last 3
$ dix last --all
$ dix last --from 300 --to 320
```

`last` shows what changed from the previous generation to the current one.
`last N` shows the N most recent changes leading up to the current generation,
comparing each generation with its predecessor, where N is a positive integer. 
If fewer changes are available, it shows all available changes.
Missing generations, such as those removed by garbage collection, are skipped.

`last --all` compares every adjacent pair of existing generations in numerical
order, including generations newer than the current one after a rollback.
`last --from A --to B` does the same for the generations numbered A to B,
inclusive; either bound can be left out. Like `--all`, ranges do not depend on
the current generation. Neither can be combined with N.

The default profile is `/nix/var/nix/profiles/system`. Use `--profile PATH` for
another unnumbered profile link, for example
`dix last 3 --profile ~/.local/state/nix/profiles/profile`.

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
