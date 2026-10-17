#!/usr/bin/env python3
"""mock-env: reports whether the configured env var and working dir arrived.

`env_probe` answers with a tool error unless `MCP_LOADTEST_ENV_MARKER`
equals `args.marker` and the process cwd equals `args.cwd`, so a run that
drops `[server.env]` or `working_dir` fails on every call.
"""

import os
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))

from _common import (  # noqa: E402
    cli_protocol_version,
    read_frame,
    respond_error,
    respond_initialize,
    respond_ok,
    respond_tools_list,
)

TOOLS = [
    {
        "name": "env_probe",
        "description": "Check the spawn env var and working directory.",
        "inputSchema": {
            "type": "object",
            "properties": {"marker": {"type": "string"}, "cwd": {"type": "string"}},
        },
    }
]


def same_dir(a, b):
    try:
        return os.path.samefile(a, b)
    except OSError:
        return False


def main():
    protocol_version = cli_protocol_version()
    while True:
        msg = read_frame()
        if msg is None:
            return
        method = msg.get("method")
        msg_id = msg.get("id")
        if method == "initialize":
            respond_initialize(msg_id, protocol_version=protocol_version)
        elif method == "notifications/initialized":
            pass
        elif method == "tools/list":
            respond_tools_list(msg_id, TOOLS)
        elif method == "tools/call":
            args = (msg.get("params") or {}).get("arguments") or {}
            marker_ok = os.environ.get("MCP_LOADTEST_ENV_MARKER") == args.get("marker")
            cwd_ok = same_dir(os.getcwd(), args.get("cwd", ""))
            text = f"marker_ok={marker_ok} cwd_ok={cwd_ok}"
            respond_ok(
                msg_id,
                {
                    "content": [{"type": "text", "text": text}],
                    "isError": not (marker_ok and cwd_ok),
                },
            )
        elif msg_id is not None:
            respond_error(msg_id, -32601, f"method not found: {method}")


if __name__ == "__main__":
    main()
