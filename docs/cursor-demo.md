# Cursor acceptance demo

This manual check is required to establish that the original result stays out of model context; the automated tests exercise the documented contract but cannot inspect Cursor's internal context assembly.

1. Build with `cargo install --path .`, then run `spill install cursor`.
2. Add the following additional server to your existing `~/.cursor/mcp.json`, substituting the absolute checkout path and preserving other servers:

   ```json
   {
     "mcpServers": {
       "spill-demo": {
         "command": "python3",
         "args": ["/absolute/path/to/spill/scripts/demo_mcp.py"]
       }
     }
   }
   ```

3. In Cursor, verify both `spill` and `spill-demo` are connected. Start a conversation and ask: “Use spill-demo list_issues, then count issues by state using Spill SQL.”
4. Inspect the tool output: it should contain a dataset descriptor with 2,000 rows and at most three preview rows, not the full array. Confirm Cursor's tool/context diagnostics show the replacement rather than all 2,000 objects. Expected aggregate: OPEN 1,000; CLOSED 1,000.
5. Run `spill list`, `spill describe <dataset>`, and `spill sql 'SELECT count(*) FROM <dataset>'` in a terminal. Count must be 2,000.
6. Ask Cursor to call `spill-demo small_result`. Its single object should remain unchanged.
7. Run `spill uninstall cursor`; verify unrelated servers and hooks remain and `spill list` still shows the dataset.
8. Remove only the `spill-demo` entry you added when finished.

The hook expects the documented `MCP:<tool_name>` identity in `postToolUse`. If a Cursor build emits another shape, retain an anonymized fixture and update the isolated adapter; do not guess undocumented fields. Until live replacement is verified on your Cursor build, do not assume the large result was excluded from model context.
