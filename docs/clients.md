# Codex and Claude Code integration

Install the client you use:

```bash
spill install codex
spill install claude
```

`claude-code` is accepted as an alias. These commands configure the local CLI clients, not Claude Desktop. All clients query the same `~/.spill/spill.duckdb` database. Restart your client after installation and approve its normal MCP/hook trust prompts when applicable.

## Hook contracts

Both new adapters accept only successful `PostToolUse` events for `mcp__<server>__<tool>`, read `tool_response` and `tool_use_id`, and ignore Spill's own tools. They use the same 32 KiB threshold, extraction, transaction and descriptor logic as Cursor. Unsupported input and storage failures return `{}` with exit status zero.

Codex does **not** support `updatedMCPToolOutput`. Its documented alternative is `continue: false` with a descriptor in `stopReason`: the model receives hook feedback in place of the original result. Unlike `decision: "block"`, this does not reject the nested tool promise in code mode. This adapter requires a Codex version implementing that behavior; it does not use `additionalContext`, which would retain the oversized result. See [Codex hooks](https://learn.chatgpt.com/docs/hooks#posttooluse).

Claude Code uses the MCP-specific `hookSpecificOutput.updatedMCPToolOutput`, with `hookEventName: "PostToolUse"`. That documented field also works on versions predating the newer all-tool `updatedToolOutput`. The adapter accepts MCP result envelopes, direct object arrays, JSON strings, and a single text content block containing an object array. Image/audio and mixed content blocks pass through; content blocks are not mistaken for dataset rows. See [Claude Code hooks](https://code.claude.com/docs/en/hooks#posttooluse).

Only model-visible output is replaced. Original responses may still exist in client logs or telemetry. Automated tests check the hook contract; verifying exclusion from actual model context requires the manual check below on your client version.

## Configuration and ownership

Codex registers `[mcp_servers.spill]` in `~/.codex/config.toml` and appends a matching hook group to `~/.codex/hooks.json`. Existing inline TOML hook definitions remain intact; Codex merges matching hooks from both sources. `CODEX_HOME` overrides the config directory. See [Codex MCP configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

Claude Code registers a global/user-scope `mcpServers.spill` in `~/.claude.json` and a PostToolUse hook group in `~/.claude/settings.json`. Project-specific servers, permissions and other settings remain intact. If `CLAUDE_CONFIG_DIR` is set, the files are `<dir>/.claude.json` and `<dir>/settings.json`. See [Claude Code MCP scopes](https://code.claude.com/docs/en/mcp#scope-hierarchy-and-precedence).

The installer backs up changed files, tracks ownership separately per client, and refuses conflicting entries. Changing the environment override after installation does not redirect an uninstall to another config: use the original directory first. Uninstall preserves the database:

```bash
spill uninstall codex
spill uninstall claude
```

## Manual acceptance check

1. Build/install Spill and run the installer for your client.
2. Register the repository's demo MCP fixture with that client, substituting an absolute path:

   ```bash
   codex mcp add spill-demo -- python3 /absolute/path/to/spill/scripts/demo_mcp.py
   # Or:
   claude mcp add --scope user spill-demo -- python3 /absolute/path/to/spill/scripts/demo_mcp.py
   ```

3. Restart the client. Confirm Spill's `query`, `list`, and `describe` tools are available.
4. Ask it to call `spill-demo.list_issues` and then use Spill SQL to count issues by state.
5. Inspect the model-facing result in the client's diagnostics: a compact descriptor should replace the 2,000-row result. Expected counts are OPEN 1,000 and CLOSED 1,000. On Codex, also check a code-mode invocation if enabled; the hook must provide the descriptor without making the nested tool call fail.
6. Confirm the table with `spill list`, `spill describe <dataset>`, and `spill sql 'SELECT count(*) FROM <dataset>'`.
7. Call `spill-demo.small_result` and confirm pass-through. Uninstall Spill and confirm unrelated configuration remains.
8. Remove the demo server with `codex mcp remove spill-demo` or `claude mcp remove --scope user spill-demo`.

No real client conversations or LLM calls are made by the automated smoke test. It exercises the actual Spill binary, generated config, hook inputs/outputs, persistence and stdio MCP in temporary directories.
