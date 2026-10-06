# npm_backend

The npm side of the package manager: running the CLI, parsing its output and
classifying versions. No UI.

## Why it exists

`npm_manager_panel`, `node_panel` and `dashboard_panel` all need the same
answers — what is installed, what is outdated, what has advisories. This
crate is the single place that knows how to ask npm and how to read what it
says, so the panels contain no npm handling of their own and the parsers can
be unit-tested against captured output.

## Using it

Functions take plain strings and return plain values or `Result<_, String>`.
The CLI-invoking ones block; run them on a background task.

```rust
let manager = npm_backend::detect_package_manager(&project_root);
let outdated = npm_backend::list_outdated(&project_root, manager.cli_name())?;
for package in &outdated {
    let kind = npm_backend::classify_update(&package.current, &package.latest);
}
```

### API

| Area | Items |
| --- | --- |
| CLI | `run_npm_cli`, `detect_package_manager` → `PackageManager` (`cli_name()`) |
| Lists | `list_installed`, `list_outdated`, `list_audit_vulns` |
| CLI output parsers | `parse_installed`, `parse_outdated`, `parse_audit` |
| Registry JSON parsers | `parse_npm_search`, `parse_npm_packument`, `parse_npm_downloads` |
| Versions | `classify_update` → `UpdateKind`, `version_matches_all`, `version_from_output` |
| Compatibility | `engine_compat` → `EngineCompat`, `node_engine_compatible`, `peer_conflicts_for`, `peer_conflicts_with_installed` |
| Security | `has_vuln` |
| Types | `NpmInstalledPkg`, `NpmOutdatedPkg`, `NpmAuditVuln`, `NpmSearchResult`, `NpmPackageDetails`, `NpmVersionEntry` |

`detect_package_manager` picks npm, pnpm, yarn or bun from the lockfile in
the project root (`pnpm-lock.yaml`, `yarn.lock`, `bun.lockb`/`bun.lock`),
defaulting to npm.

### Division of labour

- **This crate** runs the CLI and parses JSON — both the CLI's and the
  registry's.
- **The panel** makes the HTTP requests to `registry.npmjs.org` and hands the
  response bodies to the `parse_npm_*` functions here.

That split keeps the HTTP client out of this crate while leaving the parsing
shared and testable.

## Scope

Forge also aggregated findings into a host-wide security feed. That half was
not ported; only the local per-package `npm audit` list is kept (see
`parse_audit`).

`src/path_env.rs` enriches PATH on Windows so `npm` is found when the app is
launched from the GUI — see the explanation in
[`node_backend`](../node_backend/README.md#path-handling-on-windows).

## Development

```sh
cargo check -p npm_backend -j 8
cargo test -p npm_backend -j 8
```

The tests cover lockfile detection, the `npm ls` / `npm outdated` /
`npm audit` parsers (including the older output shapes), the registry parsers, and
the version, engine and peer-dependency checks. Nothing in them invokes
`npm`.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
