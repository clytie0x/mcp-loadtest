#!/usr/bin/env python3
"""mock-annotated: tools with MCP annotations, for `deadlock-probe` without
`--tool` (ADR 0030).

The tools are listed in this order, and only the last one qualifies for the
automatic choice:

  write_note  not read-only, no arguments
  search      read-only, requires `query`
  wipe        read-only and destructive, no arguments
  get_status  read-only, no arguments

Calling any tool other than `get_status` returns a JSON-RPC error, so a
probe that picked the wrong tool fails.

`--paged` serves `tools/list` in two pages: the first three tools, then
`get_status` behind `nextCursor`.
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))

from _common import (  # noqa: E402  (path manipulation above is intentional)
    read_frame,
    respond_error,
    respond_initialize,
    respond_ok,
)

OBJECT = {"type": "object"}

TOOLS = [
    {
        "name": "write_note",
        "description": "Writes a note.",
        "inputSchema": OBJECT,
        "annotations": {"readOnlyHint": False},
    },
    {
        "name": "search",
        "description": "Searches notes.",
        "inputSchema": {
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
        },
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "wipe",
        "description": "Deletes every note.",
        "inputSchema": OBJECT,
        "annotations": {"readOnlyHint": True, "destructiveHint": True},
    },
    {
        "name": "get_status",
        "description": "Reports the server status.",
        "inputSchema": {"type": "object", "properties": {}, "required": []},
        "annotations": {"readOnlyHint": True, "destructiveHint": False},
    },
]

NEXT_CURSOR = "page-2"


def tools_list_result(params):
    if "--paged" not in sys.argv:
        return {"tools": TOOLS}
    if params.get("cursor") == NEXT_CURSOR:
        return {"tools": TOOLS[3:]}
    return {"tools": TOOLS[:3], "nextCursor": NEXT_CURSOR}


def main():
    while True:
        msg = read_frame()
        if msg is None:
            return  # stdin closed
        method = msg.get("method")
        msg_id = msg.get("id")
        if msg_id is None:
            continue  # notifications get no response
        params = msg.get("params") or {}
        if method == "initialize":
            respond_initialize(msg_id)
        elif method == "tools/list":
            respond_ok(msg_id, tools_list_result(params))
        elif method == "tools/call":
            name = params.get("name")
            if name == "get_status":
                respond_ok(msg_id, {
                    "content": [{"type": "text", "text": json.dumps({"status": "ok"})}],
                })
            else:
                respond_error(msg_id, -32602, f"this fixture only answers get_status, not {name}")
        else:
            respond_error(msg_id, -32601, f"method not found: {method}")


if __name__ == "__main__":
    main()
