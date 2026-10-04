#!/usr/bin/env python3
"""Exercise the built CLI, hook, persistence and real stdio MCP in a temporary HOME."""
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/spill").resolve())

with tempfile.TemporaryDirectory(prefix="spill-smoke-") as home:
    env = dict(os.environ, HOME=home)
    # Never inherit real client config overrides into this isolated test.
    env.pop("CODEX_HOME", None)
    env.pop("CLAUDE_CONFIG_DIR", None)

    def cli(*args, stdin=None):
        return subprocess.run([binary, *args], input=stdin, text=True, capture_output=True,
                              check=True, timeout=60, env=env).stdout

    cli("install", "cursor")
    before = (Path(home) / ".cursor/hooks.json").read_bytes()
    cli("install", "cursor")
    assert (Path(home) / ".cursor/hooks.json").read_bytes() == before
    assert json.loads(cli("list")) == []
    server = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, env=env)
    selector = selectors.DefaultSelector()
    selector.register(server.stdout, selectors.EVENT_READ)

    def send(message):
        server.stdin.write((json.dumps(message) + "\n").encode())
        server.stdin.flush()

    def rpc(request_id, method, params):
        send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        while True:
            assert selector.select(30), f"MCP timed out on {method}"
            line = server.stdout.readline()
            assert line, f"MCP exited: {server.poll()}"
            response = json.loads(line)
            if response.get("id") == request_id:
                assert "error" not in response, response
                return response["result"]

    try:
        rpc(1, "initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                               "clientInfo": {"name": "spill-smoke", "version": "1"}})
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        listed = rpc(2, "tools/list", {})
        assert {tool["name"] for tool in listed["tools"]} == {"query", "list", "describe"}
        rows = [{"id": n, "state": "OPEN" if n % 2 == 0 else "CLOSED"} for n in range(2000)]
        event = {"hook_event_name": "postToolUse", "tool_name": "MCP:demo_list_issues",
                 "tool_use_id": "smoke-1", "tool_output": json.dumps(rows)}
        output = cli("hook", "cursor", stdin=json.dumps(event))
        assert len(output) < 8192 and "updated_mcp_tool_output" in json.loads(output)
        dataset = json.loads(cli("list"))[0]["dataset"]
        assert json.loads(cli("describe", dataset))["rows"] == 2000
        result = rpc(3, "tools/call", {"name": "query", "arguments": {
            "sql": f"SELECT state, count(*) AS n FROM {dataset} GROUP BY state ORDER BY state"}})
        assert not result.get("isError"), result
        assert result["structuredContent"]["result"]["rows"] == [["CLOSED", 1000], ["OPEN", 1000]]
        for request_id, tool, args in [(4, "list", {}), (5, "describe", {"dataset": dataset})]:
            assert not rpc(request_id, "tools/call", {"name": tool, "arguments": args}).get("isError")
        for request_id, sql in [(6, f"DROP TABLE {dataset}"), (7, "SELECT * FROM range(501)"),
                                (8, "SELECT repeat('x', 40000)")]:
            assert rpc(request_id, "tools/call", {"name": "query", "arguments": {"sql": sql}})["isError"]
        for raw in ["[{\"id\":1}]", "x" * 100000, json.dumps(["x" * 100000]), "malformed"]:
            event["tool_output"] = raw
            assert json.loads(cli("hook", "cursor", stdin=json.dumps(event))) == {}
        assert json.loads(cli("sql", f"SELECT count(*) FROM {dataset}"))["rows"] == [[2000]]
    finally:
        server.stdin.close()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
        selector.close()
    cli("uninstall", "cursor")
    assert "spill" not in json.loads((Path(home) / ".cursor/mcp.json").read_text())["mcpServers"]
    assert len(json.loads(cli("list"))) == 1

    for client in ("codex", "claude"):
        cli("install", client)
        cli("install", client)
        response = {"content": [{"type": "text", "text": json.dumps(rows)}]}
        event = {"hook_event_name": "PostToolUse", "tool_name": "mcp__demo__list_issues",
                 "tool_use_id": f"{client}-call", "tool_response": response}
        replacement = json.loads(cli("hook", client, stdin=json.dumps(event)))
        if client == "codex":
            assert replacement["continue"] is False
            assert "Rows: 2000" in replacement["stopReason"]
            assert "decision" not in replacement
            assert (Path(home) / ".codex/config.toml").exists()
            hooks_file = Path(home) / ".codex/hooks.json"
        else:
            assert "Rows: 2000" in replacement["hookSpecificOutput"]["updatedMCPToolOutput"]["content"][0]["text"]
            assert "spill" in json.loads((Path(home) / ".claude.json").read_text())["mcpServers"]
            hooks_file = Path(home) / ".claude/settings.json"
        assert len(json.loads(hooks_file.read_text())["hooks"]["PostToolUse"]) == 1
        event["tool_response"] = [{"id": 1}]
        assert json.loads(cli("hook", client, stdin=json.dumps(event))) == {}
        cli("uninstall", client)
        assert json.loads(hooks_file.read_text())["hooks"]["PostToolUse"] == []
        cli("uninstall", client)
    assert len(json.loads(cli("list"))) == 3
print("PASS: Cursor/Codex/Claude install, hooks, persistence, CLI, MCP tools, read-only queries, limits, uninstall")
