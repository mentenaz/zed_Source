# SQL Language Server Extension TODO

This document tracks the phased work for adding first-class SQL editing support
to the Zed fork through the `sqls` language server. The extension package will
live under `extensions/sqls`; `crates/extension` provides the host-side API and
manifest/runtime support it consumes.

## Goals

- Provide SQL completion in ordinary `*.sql` files.
- Support the LSP features exposed by `sqls`, including hover, diagnostics,
  formatting, signature help, and definition/navigation where schema metadata
  allows it.
- Preserve useful keyword and syntax assistance when no database connection is
  configured.
- Keep the database workbench and normal editor workflows compatible through a
  shared configuration and schema model over time.

## Phase 1: Extension scaffold and LSP launch

- [x] Create `extensions/sqls/Cargo.toml` using the existing extension crate
      conventions.
- [x] Create `extensions/sqls/extension.toml` and register the `SQL` language
      server as `sqls`.
- [x] Create the Rust extension entry point and register it with
      `zed_extension_api`.
- [x] Implement `language_server_command` to launch an installed `sqls`
      executable.
- [x] Support a deterministic binary lookup order:
      `lsp.sqls.binary.path` setting, worktree `which("sqls")`, then a clear
      error. The explicit path setting is required because WebAssembly
      extensions cannot read the host process environment directly.
- [x] Implement the minimum workspace/initialization configuration methods,
      returning no database connection by default.
- [x] Build the extension and verify that a development extension can be
      installed and started for a normal `.sql` file.
- [ ] Verify that the first LSP session starts successfully and that keyword
      completion is returned.

### Binary path configuration

When `sqls` is not on the worktree PATH, configure its absolute executable
path in Zed settings:

```json
{
  "lsp": {
    "sqls": {
      "binary": {
        "path": "C:\\Users\\<user>\\go\\bin\\sqls.exe"
      }
    }
  }
}
```

## Phase 2: Project configuration and installation

- [ ] Define the supported project configuration file and document its schema.
- [ ] Load optional `sqls` connection settings without embedding credentials
      or inventing silent defaults.
- [ ] Pass workspace configuration and initialization options to `sqls`
      according to its supported protocol.
- [ ] Add optional managed binary download/install with platform and
      architecture validation.
- [ ] Report missing binaries, invalid configuration, and failed downloads
      explicitly through the extension installation status/error path.
- [ ] Test configuration on Windows, macOS, and Linux where supported.

## Phase 3: SQL editing behavior

- [ ] Verify completion, hover, signature help, diagnostics, formatting, and
      definition/navigation behavior in ordinary `*.sql` files.
- [ ] Test files containing DDL followed by queries in the same document.
- [ ] Document which features require live schema metadata and which work
      without a database connection.
- [ ] Add completion label customization only if the default Zed rendering
      does not present SQL items clearly.
- [ ] Add focused extension tests for command construction, configuration
      precedence, and failure reporting.

## Phase 4: Database workbench integration

- [ ] Define the shared database connection/configuration contract.
- [ ] Reuse the database panel's normalized `SchemaIR` as the source of truth
      for workbench schema data.
- [ ] Decide how the workbench publishes or generates `sqls` connection
      configuration without exposing secrets unnecessarily.
- [ ] Add actions to open SQL files, tables, and columns from the workbench.
- [ ] Keep workbench introspection and editor LSP behavior consistent when a
      connection changes or becomes unavailable.

## Phase 5: Schema authoring workflow

- [ ] Support an explicit development-database workflow for schemas that are
      being authored but are not yet deployed.
- [ ] Define how migrations or bootstrap SQL are applied to that development
      database.
- [ ] Refresh workbench and LSP schema context after successful schema changes.
- [ ] Clearly distinguish unsaved/speculative DDL from schema confirmed by the
      database.
- [ ] Evaluate whether a future SQL-specific parser/index is needed for
      same-file speculative definitions that `sqls` cannot resolve.

## Acceptance criteria

- Opening an ordinary `.sql` file can start the `sqls` LSP through the
  extension.
- Typing SQL keywords produces completion suggestions without a database.
- A configured development database enables schema-aware table/column
  completion and any supported navigation features.
- Missing binaries and invalid configuration produce actionable errors.
- The database workbench remains independent of the LSP and can share
  connection/schema context through an explicit, documented contract.
