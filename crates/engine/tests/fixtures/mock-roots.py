#!/usr/bin/env python3
"""Sends the client requests of its own and blocks until they are answered.

Mirrors servers such as mcp-shrimp-task-manager, which send `roots/list`
after `notifications/initialized` and hold `tools/list` until the client
replies. A client that reads that request as the response to its own call
fails; a correct one answers it (here: method-not-found for `roots/list`,
an empty result for `ping`) and keeps waiting for its response.

The `echo` tool reports what the two requests got back.
"""
import json
from _common import read_frame, respond_initialize, respond_ok, respond_tools_list, write_frame

REQUESTS = {"srv-roots": "roots/list", "srv-ping": "ping"}


def main():
    replies = {}
    held = []
    while True:
        msg = read_frame()
        if msg is None:
            break
        if msg.get("id") in REQUESTS and "method" not in msg:
            replies[msg["id"]] = msg
            if len(replies) == len(REQUESTS):
                for request in held:
                    answer(request, replies)
                held = []
            continue
        method = msg.get("method")
        if method == "initialize":
            respond_initialize(msg["id"])
        elif method == "notifications/initialized":
            for request_id, request_method in REQUESTS.items():
                write_frame({"jsonrpc": "2.0", "id": request_id, "method": request_method})
        elif len(replies) < len(REQUESTS):
            held.append(msg)
        else:
            answer(msg, replies)


def answer(msg, replies):
    if msg.get("method") == "tools/list":
        respond_tools_list(msg["id"], tools=[{"name": "echo", "inputSchema": {"type": "object"}}])
    elif msg.get("method") == "tools/call":
        roots = replies["srv-roots"].get("error", {}).get("code")
        ping = "ok" if replies["srv-ping"].get("result") == {} else "bad"
        respond_ok(msg["id"], {"content": [{"type": "text", "text": f"roots={roots} ping={ping}"}]})


if __name__ == "__main__":
    main()
