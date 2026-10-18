#!/usr/bin/env python3
"""mock-http-server: Streamable HTTP MCP server (simple JSON variant).

POST / accepts a JSON-RPC request body and returns the response as
`application/json`. Notifications (no `id`) return 204 No Content.

Stdlib only. Bind to 127.0.0.1:<port>; emit one stdout line
`LISTENING: 127.0.0.1:<port>` (with the OS-assigned port when --port 0)
so test harnesses can read where to connect.

With --store racy|safe it also serves `append {item}` and `read`, backed by
one in-memory list shared by every client. `racy` copies the list, pauses,
and writes the copy back, so concurrent appends overwrite each other;
`safe` appends under a lock.
"""

import argparse
import json
import signal
import socketserver
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STORE_MODE = None
ITEMS = []
ITEMS_LOCK = threading.Lock()

STORE_TOOLS = [
    {
        "name": "append",
        "description": "Append one item to the shared store.",
        "inputSchema": {
            "type": "object",
            "properties": {"item": {"type": "string"}},
            "required": ["item"],
        },
    },
    {
        "name": "read",
        "description": "Return every stored item, one per line.",
        "inputSchema": {"type": "object"},
    },
]


def store_call(name, args):
    """Run a store tool; returns the result text, or None for other tools."""
    global ITEMS
    if name == "append":
        item = str(args.get("item", ""))
        if STORE_MODE == "racy":
            snapshot = list(ITEMS)
            time.sleep(0.02)
            snapshot.append(item)
            ITEMS = snapshot
        else:
            with ITEMS_LOCK:
                ITEMS.append(item)
        return "ok"
    if name == "read":
        return "\n".join(ITEMS)
    return None


def dispatch(msg):
    """Return a JSON-RPC response dict for `msg`, or None for notifications."""
    method = msg.get("method")
    msg_id = msg.get("id")
    if msg_id is None:
        # notification — no response (e.g. notifications/initialized)
        return None
    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-http", "version": "0.0.0"},
            },
        }
    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo arguments back as JSON text.",
                        "inputSchema": {"type": "object"},
                    }
                ]
                + (STORE_TOOLS if STORE_MODE else [])
            },
        }
    if method == "tools/call":
        params = msg.get("params", {})
        args = params.get("arguments", {})
        text = store_call(params.get("name"), args) if STORE_MODE else None
        if text is not None:
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"content": [{"type": "text", "text": text}]},
            }
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {"content": [{"type": "text", "text": json.dumps(args)}]},
        }
    return {
        "jsonrpc": "2.0",
        "id": msg_id,
        "error": {"code": -32601, "message": f"method not found: {method}"},
    }


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):  # noqa: N802  (BaseHTTPRequestHandler API)
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b""
        try:
            msg = json.loads(raw.decode("utf-8")) if raw else {}
        except (UnicodeDecodeError, json.JSONDecodeError):
            self.send_response(400)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"invalid json\n")
            return

        response = dispatch(msg)
        if response is None:
            # notification — 204 No Content
            self.send_response(204)
            self.end_headers()
            return

        body = json.dumps(response).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):  # silence per-request stderr logging
        pass


class _Server(ThreadingHTTPServer):
    """`http.server.HTTPServer.server_bind()` calls `socket.getfqdn(host)`, a
    reverse-DNS lookup that can block ~30s on macOS CI runners (and is not
    needed for a loopback test mock). Bind via the plain TCPServer path and
    set `server_name`/`server_port` directly so `LISTENING:` is emitted
    immediately on every platform.
    """

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = host
        self.server_port = port


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--store", choices=["racy", "safe"])
    args = parser.parse_args()
    global STORE_MODE
    STORE_MODE = args.store

    server = _Server((args.host, args.port), Handler)
    host, port = server.server_address[0], server.server_address[1]
    sys.stdout.write(f"LISTENING: {host}:{port}\n")
    sys.stdout.flush()

    stop = threading.Event()

    def shutdown(_signum=None, _frame=None):
        stop.set()
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)
    try:
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
