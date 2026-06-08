# Changelog

This is a changelog of the `dix` repository. It follows the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format and adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 2.0.1

## Added

- Added proper CI tests for all supported systems, formatting and checks.

## Fixed

- Fixed build failures on aarch64-darwin and failing tests on x86_64-linux.

## 2.0.0

### Added

- Added the `dix-diff` crate for the pure package/version diff engine. This
  crate contains the core diff model and algorithms, but does not depend on any
  Nix-specific data sources or APIs. It is intended for users who want to
  perform package/version diffs without needing to query the Nix store, and for
  users who want to build on top of the diff engine with their own custom data
  sources or APIs.
- Added public diff event types for version-level changes: `VersionDiff`,
  `VersionAmount`, and the `AmountChanged` event for count-only changes.
- Added `PackageSnapshot` and `diff_snapshots` for callers that want to diff
  package/version data without querying the Nix store.
- Added root `dix` re-exports for the main diff model types: `DiffStatus`,
  `Version`, `VersionAmount`, and `VersionDiff`.
- Added exact closure path statistics to reports. Human output now shows old
  and new closure path counts plus exact added and removed store path counts,
  and JSON output includes the same data in the `paths` object.
- Added package size deltas to reports. Human output shows significant
  per-package NAR size changes, and JSON diff entries include old, new, and
  delta size fields in bytes.

### Changed

- Changed the repository layout from a single Cargo package to a workspace.
  Source installs now use `cargo install --path crates/dix`.
- Changed the Rust diff model from separate `old` and `new` version lists to a
  `versions` event list.
- Changed diff statuses from `DiffStatus::Changed(Change::...)` to flat variants
  such as `Changed`, `Mixed`, `Upgraded`, and `Downgraded`.
- Changed version counts to use `VersionAmount` and `VersionDiff::AmountChanged`
  instead of storing counts on `Version`.
- Changed version parsing and status classification to treat Git short or full
  hash suffixes as unordered versions. These changes are now reported as
  changed instead of arbitrary upgrades or downgrades.
- Changed report querying from `DiffReport::query(...)` to the
  `query_diff_report(...)` free function.
- Changed `DiffReport` and `PathStats` to expose read-only accessor methods
  instead of public fields.
- Changed JSON output. Diff entries now contain tagged `versions` events and
  `has_omitted_versions` instead of `old`, `new`, and `has_common_versions`,
  and reports include exact closure path statistics in `paths`.

### Removed

- Removed the old `dix::diff` and `dix::version` module paths.
- Removed `generate_diffs_from_paths(...)` and `match_version_lists(...)` from
  the `dix` public API.
- Removed the public `DiffReport::between(...)` constructor. Store-backed
  reports should be built with `query_diff_report(...)`; pure package/version
  diffs should use `dix-diff`.
- Removed the `Change` enum.
- Removed `Version.amount`.
- Removed direct serde serialization of the public diff model types.

### Breaking Changes

- `cargo install --path .` no longer works because the repository root is now a
  workspace manifest. Use `cargo install --path crates/dix` instead.
- Rust users importing `dix::diff` or `dix::version` must update their imports.
  The main diff model types are re-exported from `dix`; lower-level engine APIs
  live in `dix-diff`.
- Rust users constructing or matching diffs must migrate from the old
  `old`/`new` list model to the new `VersionDiff` event model.
- Rust users matching `DiffStatus::Changed(Change::...)` must migrate to the new
  flat `DiffStatus` variants.
- Rust users reading or writing `Version.amount` must use `VersionAmount` or
  `VersionDiff::AmountChanged` instead.
- Rust users calling `DiffReport::query(...)` must call `query_diff_report(...)`
  instead.
- Rust users calling `DiffReport::between(...)` must use
  `query_diff_report(...)` for store-backed reports or `dix-diff` for pure
  package/version diffs.
- Rust users reading `DiffReport` or `PathStats` fields directly must use the
  new accessor methods instead. Direct construction of `DiffReport` is no
  longer supported.
- Rust users implementing `dix::store::StoreBackend` must implement
  `query_closure_path_info(...)`. The default `query_closure_size(...)`
  implementation now derives aggregate closure size from that path info.
- JSON consumers must update to the new `--output json` schema. The old `old`,
  `new`, and `has_common_versions` fields are no longer emitted, and exact
  closure path statistics are emitted in the new `paths` object. Diff entries
  also include `size_old`, `size_new`, and `size_delta` fields.
