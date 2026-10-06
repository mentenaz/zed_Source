# python_backend

Python environment detection, project scanning and package logic, with no
UI.

## Why it exists

It holds everything `python_panel` and `python_manager_panel` need to know
about a Python project — interpreter and pip versions, installed and outdated
packages, which framework the project uses, which dependencies are missing —
as plain functions that can be shared between the two panels and tested
without a window.

Ported from Forge alongside [`python_panel`](../python_panel/README.md).

## Using it

Functions take plain strings and return plain values or `Result<_, String>`.
Those that run `python`/`pip` block; call them from a background task.

```rust
let version = python_backend::query_python("python")?;
let packages = python_backend::list_packages("python")?;
let projects = python_backend::scan_python_projects(&root, 5);
```

### API

| Area | Items |
| --- | --- |
| Runtime | `query_python(exe)`, `query_pip(exe)` |
| Packages | `list_packages(exe)`, `list_outdated(exe)`, `classify_update` → `UpdateKind` |
| Project scan | `scan_python_projects(root, depth)` → `Vec<DetectedPythonProject>`, `scan_python_project(root)` → `PythonProjectScan` |
| Environments | `EnvMarker` (`file_name()`, `create_command(exe)`) |
| Frameworks | `detect_framework`, `FRAMEWORK_MARKERS`, `PYTHON_ENTRY_POINTS` |
| Dependencies | `find_missing_deps`, `count_requirements` |
| PyPI JSON parsers | `parse_pypi_json` → `PyPiPackageInfo`, `parse_pypi_vulnerabilities` → `Vec<PyPiVulnerability>` |

`EnvMarker` records which file a project declares its dependencies in —
`requirements.txt`, `pyproject.toml` or `Pipfile`, checked in that order —
and `create_command` returns the one-line command that bootstraps an
environment from it, using the venv interpreter path for the current OS
(`venv\Scripts\python.exe` on Windows, `venv/bin/python` elsewhere).

### Division of labour

The panels make the HTTP requests to `pypi.org`; this crate only parses the
response bodies. That keeps the HTTP client out of the backend.

## Things to know about PyPI

- There is no full-text search API (it was removed in 2018), so only
  exact-name lookups via `https://pypi.org/pypi/<name>/json` are possible.
- There is no bulk audit endpoint. Vulnerability data comes back per package
  and version, which is why the panels scan on demand rather than
  automatically.

`src/path_env.rs` enriches PATH on Windows so `python` is found when the app
is launched from the GUI — see
[`node_backend`](../node_backend/README.md#path-handling-on-windows).

## Development

```sh
cargo check -p python_backend -j 8
cargo test -p python_backend -j 8
```

The tests cover the pip and PyPI parsers, version classification, project
and framework detection, and the requirements helpers (against temporary
directories). Nothing in them invokes `python` or `pip`.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
