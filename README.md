# Spill

**Keep large tool results out of the context window.**

Spill automatically turns oversized MCP tool results into local DuckDB tables so AI agents can query them instead of loading the entire result into context.

Install with Homebrew (one-time tap setup):

```bash
brew tap spill-ai/spill https://github.com/spill-ai/spill
brew trust --formula spill-ai/spill/spill
brew install spill
spill install cursor
```

The explicit tap URL uses this repository directly; no separate tap repository is required. On a fresh Homebrew installation, the tap step is required because Spill is not in `homebrew/core`. Homebrew 6+ also requires the one-time formula trust command shown above (omit it on older versions). After setup, use `brew install spill` and `brew upgrade spill` normally.

Or choose Codex or Claude Code:

```bash
spill install codex
spill install claude
```

Then keep using your client normally. The installer registers the absolute executable path, so the client does not need Cargo's bin directory on its PATH. Restart the client to load its MCP server and hooks. `claude-code` is an alias for `claude`; Claude Desktop is not supported.

```text
GitHub MCP → 18,412 issues → Spill → DuckDB table → small descriptor
                                      ↑
                               spill.query(SQL)
```

```sql
SELECT state, COUNT(*)
FROM spill_list_issues_a81f32
GROUP BY state;
```

No cloud account. No embeddings. No new data infrastructure. Spill makes no network calls at runtime.

## Installation

The stable Homebrew formula pins a source commit and builds embedded DuckDB. Homebrew installs Rust as a build dependency; a C++ toolchain is also needed (Xcode Command Line Tools on macOS). The first build can take several minutes. macOS and Linux are the V0 targets.

To build directly from a checkout:

```bash
cargo install --locked --path .
```

For development builds through Homebrew, use `brew install --HEAD spill-ai/spill/spill`. See [Homebrew packaging](docs/homebrew.md) for verification and updating the stable version.

```bash
spill install cursor
spill uninstall cursor
```

Each installer preserves unrelated servers, hooks, and settings, backs up existing files before changes, and is idempotent:

| Client | MCP configuration | Hook configuration | Result replacement |
| --- | --- | --- | --- |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/hooks.json` | `postToolUse.updated_mcp_tool_output` |
| Codex | `~/.codex/config.toml` | `~/.codex/hooks.json` | `PostToolUse`, `continue: false`, descriptor in `stopReason` |
| Claude Code | `~/.claude.json` (user scope) | `~/.claude/settings.json` | `PostToolUse.hookSpecificOutput.updatedMCPToolOutput` |

Codex TOML comments and unrelated settings are preserved. `CODEX_HOME` and `CLAUDE_CONFIG_DIR` overrides are supported; with the latter, Claude's MCP file is `<CLAUDE_CONFIG_DIR>/.claude.json`. Paths must be absolute. Install and uninstall using the same configuration directory.

Invalid configuration or a conflicting `spill` server causes an error before the client files are modified. Ownership manifests at `~/.spill/<client>-install.json` let uninstall remove only entries Spill added and preserve entries the user subsequently modified. Keep those manifests until uninstall. Reinstall after moving the executable. Use `spill uninstall codex` or `spill uninstall claude` for the other clients.

Codex must support the documented PostToolUse `continue: false` feedback behavior. Older versions without it do not provide the same result-replacement guarantee. Hook trust/approval and disabled-hook settings remain under the client's control; the installer does not override them. See [client integration verification](docs/clients.md).
Uninstall keeps the database and backups. The test suite uses temporary homes and never modifies your actual client configuration.

## Commands

```bash
spill list
spill describe spill_list_issues_a81f32
spill sql 'SELECT state, count(*) FROM spill_list_issues_a81f32 GROUP BY state'
spill hook cursor  # also: codex, claude; one JSON event in and response out
spill mcp          # stdio MCP server, normally launched by Cursor
```

The MCP server exposes exactly `query`, `list`, and `describe`. Query responses have `columns` and `rows`, where each row is an array in column order, preserving duplicate SQL column names. MCP structured responses wrap the result in `{"result": ...}` and also include a text representation for compatible clients.

## What spills

The serialized tool output must be at least **32 KiB** and contain an unambiguous nonempty array of JSON objects. Supported shapes are a direct array, one MCP text block containing that array as JSON, or an array in `structuredContent` (with an absent/empty or equivalent text representation). Arbitrary nested wrappers, mixed/binary content blocks, strings, malformed JSON, and tool errors pass through unchanged. Spill does not recursively search for arrays inside unrelated objects.

Columns use `BOOLEAN`, `BIGINT`, `DOUBLE`, or `VARCHAR`. Nested objects, arrays, oversized integers and mixed types are stored losslessly as JSON text in `VARCHAR`; missing values become SQL NULL. For nested data use DuckDB JSON functions or casts. Integer/float mixtures also use JSON text to avoid rounding large integers. Column names are quoted; empty, NUL-containing, or case-colliding names fail open. Arrays of empty objects use a nullable `_spill_empty_object` column to retain row count.

Data lives in **`~/.spill/spill.duckdb`**. `_spill_datasets` records each table, source tool, call ID, UTC creation time, row count, original byte count, and `original_json`. The latter preserves the complete original payload, including wrappers and missing-versus-null distinctions. Table creation, row insertion and metadata commit together.

Descriptors include at most three short preview rows, with strict schema and preview budgets. Unsupported results and failures return `{}`, leaving the client's original output intact. Diagnostic output goes to stderr.

## Query policy

Queries must begin with SELECT, WITH, SHOW, DESCRIBE or EXPLAIN. A SQL tokenizer rejects multiple statements and mutation keywords outside strings, quoted identifiers and comments. Independently, DuckDB opens the database in **read-only mode**, with external access and automatic extension installation/loading disabled. Query output is capped at **500 rows and 32 KiB**; oversized results return an error rather than a misleading partial answer. List and describe output is also bounded.

The server opens/closes the database for each tool call, allowing subsequent hook processes to write while the MCP process stays alive. V0 assumes low concurrency; a simultaneous conflicting DuckDB connection fails open in the hook and returns an error in the query tool. There is no daemon, cleanup policy, caching, or background service. Output limits do not impose a SQL execution timeout or memory budget.

## Development and verification

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
python3 scripts/smoke.py target/debug/spill
```

The smoke test uses a temporary home directory and exercises the real CLI and stdio MCP transport while the hook writes a dataset. CI runs the checks on Linux and macOS. Follow [the Cursor acceptance demo](docs/cursor-demo.md) to validate model-visible replacement in a live Cursor conversation.

Cursor integration follows the documented [`postToolUse` replacement contract](https://cursor.com/docs/hooks), using `tool_output`, `tool_use_id`, the MCP matcher, and `updated_mcp_tool_output`. `afterMCPExecution` is observational and is not used for replacement.

Spill supports Cursor, Codex and Claude Code. It does not proxy MCP servers, summarize results, or provide cloud storage.

Apache-2.0 licensed.
