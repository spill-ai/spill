#!/usr/bin/env python3
"""Local, dependency-free MCP fixture for the manual Cursor acceptance demo."""
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"],
                  "capabilities": {"tools": {}}, "serverInfo": {"name": "spill-demo", "version": "1"}}
    elif method == "ping":
        result = {}
    elif method == "tools/list":
        result = {"tools": [{"name": "list_issues", "description": "Return 2000 example issues for testing Spill.",
                            "inputSchema": {"type": "object", "properties": {}}},
                           {"name": "small_result", "description": "Return a small result for testing pass-through.",
                            "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call" and request["params"]["name"] in ("list_issues", "small_result"):
        count = 2000 if request["params"]["name"] == "list_issues" else 1
        rows = [{"id": n, "title": f"Example issue {n}", "state": "OPEN" if n % 2 == 0 else "CLOSED"}
                for n in range(count)]
        result = {"content": [{"type": "text", "text": json.dumps(rows)}], "isError": False}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unknown method/tool"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
