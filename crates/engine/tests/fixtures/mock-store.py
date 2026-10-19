#!/usr/bin/env python3
"""mock-store: an append/read store for the lost_update detector.

    mock-store.py --mode racy|safe|memory|last|refusing|closed [--store PATH]

PATH defaults to $MOCK_STORE_PATH; a relative PATH resolves against the
working directory.

- racy:   read-modify-write of a JSON list in PATH with no lock and a short
          pause in between, so concurrent processes overwrite each other.
- safe:   one file per item under the directory PATH, published with an
          atomic rename, so nothing is lost. (O_APPEND is not atomic across
          processes on Windows, so it cannot model a safe store there.)
- memory: a per-process list; other sessions never see it.
- last:   every append replaces the whole store with its own item, so only
          the most recent write survives.
- refusing: like safe, but refuses with an isError result every item whose
          lost_update marker has an odd sequence number, the way a server
          with optimistic concurrency refuses a conflicting write.
- closed: refuses every append with an isError result.
"""

import json
import os
import pathlib
import re
import sys
import time

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
        "name": "append",
        "description": "Append one item to the store.",
        "inputSchema": {
            "type": "object",
            "properties": {"item": {"type": "string"}},
            "required": ["item"],
        },
        "annotations": {"readOnlyHint": False, "destructiveHint": False},
    },
    {
        "name": "read",
        "description": "Return every stored item, one per line.",
        "inputSchema": {"type": "object"},
        "annotations": {"readOnlyHint": True},
    },
]


def arg(name, default=None):
    if name in sys.argv:
        return sys.argv[sys.argv.index(name) + 1]
    return default


MODE = arg("--mode", "safe")
STORE = arg("--store", os.environ.get("MOCK_STORE_PATH"))
MEMORY = []
WRITES = 0
CONFLICT = "REVISION_CONFLICT: the store changed after it was read.\nRead it again and retry."


def refuses(item):
    if MODE == "closed":
        return True
    if MODE != "refusing":
        return False
    seq = re.search(r"s(\d{4})x$", item)
    return seq is not None and int(seq.group(1)) % 2 == 1


def append(item):
    if MODE == "memory":
        MEMORY.append(item)
    elif MODE == "last":
        tmp = pathlib.Path(f"{STORE}.{os.getpid()}.tmp")
        tmp.write_text(json.dumps([item]))
        # Windows refuses a rename while another process renames onto the
        # same target; retry briefly instead of failing the write.
        for _ in range(50):
            try:
                os.replace(tmp, STORE)
                break
            except PermissionError:
                time.sleep(0.01)
    elif MODE == "racy":
        path = pathlib.Path(STORE)
        try:
            items = json.loads(path.read_text()) if path.exists() else []
        except (OSError, json.JSONDecodeError):
            # Another writer is mid-write; losing its data is the bug we model.
            items = []
        time.sleep(0.02)
        items.append(item)
        try:
            path.write_text(json.dumps(items))
        except OSError:
            # Windows can refuse a write that races another; acknowledge anyway,
            # which is exactly an acknowledged-but-lost update.
            pass
    else:
        global WRITES
        WRITES += 1
        directory = pathlib.Path(STORE)
        directory.mkdir(parents=True, exist_ok=True)
        name = f"{os.getpid()}-{WRITES}"
        tmp = directory / (name + ".tmp")
        tmp.write_text(item)
        os.replace(tmp, directory / (name + ".item"))


def read_all():
    if MODE == "memory":
        return list(MEMORY)
    path = pathlib.Path(STORE)
    if not path.exists():
        return []
    if MODE in ("racy", "last"):
        try:
            return json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            # A concurrent writer truncated the file mid-read.
            return []
    return [item.read_text() for item in sorted(path.glob("*.item"))]


def main():
    protocol_version = cli_protocol_version()
    while True:
        msg = read_frame()
        if msg is None:
            return
        method = msg.get("method")
        msg_id = msg.get("id")
        if msg_id is None:
            continue
        if method == "initialize":
            respond_initialize(msg_id, protocol_version=protocol_version)
        elif method == "tools/list":
            respond_tools_list(msg_id, TOOLS)
        elif method == "tools/call":
            params = msg.get("params", {})
            name = params.get("name")
            args = params.get("arguments", {})
            if name == "append" and refuses(str(args.get("item", ""))):
                respond_ok(
                    msg_id,
                    {"content": [{"type": "text", "text": CONFLICT}], "isError": True},
                )
            elif name == "append":
                append(str(args.get("item", "")))
                respond_ok(msg_id, {"content": [{"type": "text", "text": "ok"}]})
            elif name == "read":
                text = "\n".join(read_all())
                respond_ok(msg_id, {"content": [{"type": "text", "text": text}]})
            else:
                respond_error(msg_id, -32602, f"unknown tool: {name}")
        else:
            respond_error(msg_id, -32601, f"method not found: {method}")


if __name__ == "__main__":
    main()
