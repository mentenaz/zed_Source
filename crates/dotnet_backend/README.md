# dotnet_backend

.NET SDK detection, project scanning and NuGet parsing, with no UI.

## Why it exists

`dotnet_panel`, `nuget_manager_panel` and `dashboard_panel` share one
understanding of a .NET project: where the `.csproj` files are, which
packages they reference, which are outdated or vulnerable. This crate owns
that logic as plain functions so the panels stay thin and the parsers can be
tested against captured XML and JSON.

Ported from Forge alongside [`dotnet_panel`](../dotnet_panel/README.md).

## Using it

Functions take plain strings and return plain values or `Result<_, String>`.
Those that invoke `dotnet` block; call them from a background task.

```rust
let projects = dotnet_backend::scan_dotnet_projects(&root, dotnet_backend::MAX_SCAN_DEPTH);
if let Some(csproj) = dotnet_backend::find_csproj(&root) {
    let installed = dotnet_backend::read_installed_packages(&csproj);
}
let outdated = dotnet_backend::list_outdated(project_dir)?;
```

### API

| Area | Items |
| --- | --- |
| CLI | `run_dotnet_args(args, cwd)`, `query_runtime` |
| Project scan | `scan_dotnet_projects(root, depth)` → `Vec<DotnetProject>`, `find_csproj`, `resolve_csproj` |
| Installed packages | `read_installed_packages(csproj)`, `parse_package_refs` (`<PackageReference>`), `parse_packages_config` (`packages.config`) |
| Outdated / vulnerable | `list_outdated`, `list_vulnerable`, `parse_outdated_json`, `parse_vulnerable_json` |
| Versions | `classify_update` → `UpdateKind` (`label()`), `fmt_downloads` |
| NuGet registry | `nuget_registration_url`, `nuget_flat_readme_url`, `parse_nuget_search`, `registration_items_from_pages`, `parse_nuget_details` |
| Types | `InstalledPackage`, `OutdatedPackage`, `VulnerablePackage`, `NugetSearchResult`, `NugetPackageDetails`, `NugetDependencyGroup`, `NugetDependency` |
| Limits | `MAX_SCAN_DEPTH` (5), `SCAN_SKIP_DIRS` |

### Division of labour

The host panel makes the HTTP calls to the NuGet registry. This crate builds
the URLs and parses the responses, so the parsing is shared and testable
while the HTTP client stays out of the backend.

## Things to know about the `dotnet` CLI

`dotnet list package` refuses to run in a directory that holds more than one
project file. Callers therefore pass the directory of one specific `.csproj`
(its parent), not the workspace root. This is why the NuGet manager is
single-project.

`src/path_env.rs` enriches PATH on Windows so `dotnet` is found when the app
is launched from the GUI — see
[`node_backend`](../node_backend/README.md#path-handling-on-windows).

## Development

```sh
cargo check -p dotnet_backend -j 8
cargo test -p dotnet_backend -j 8
```

The tests cover the `.csproj` / `packages.config` parsers, the
`dotnet list package` and NuGet registry JSON parsers, version
classification and the project scanner (against temporary directories).
Nothing in them invokes `dotnet`.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
