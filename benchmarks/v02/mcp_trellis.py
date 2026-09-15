#!/usr/bin/env python3
"""MCP stdio adapter for the trellis CLI (v0.2 Phase 1).

Thin plumbing only: JSON-RPC over stdio -> trellis CLI subprocess calls.
No logic beyond argument/env routing; all Trellis behavior lives in the
CLI. Python 3 stdlib only.

Env:
  TRELLIS_BIN   path to the trellis binary (default: target/debug/trellis)
  TRELLIS_REPO  repo root passed as --repo
  TRELLIS_STORE store path passed as --store
"""

import json
import os
import subprocess
import sys

PROTOCOL_VERSION = "2024-11-05"

TOOLS = [
    {
        "name": "trellis_status",
        "description": (
            "List published Trellis artifacts and their current validity "
            "(valid/stale/unknown) against the current repository state."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "trellis_retrieve",
        "description": (
            "Retrieve a reusable artifact by key. Returns a verdict "
            "(valid|stale|unknown) plus the artifact value ONLY when valid. "
            "Stale/unknown artifacts are never returned as trusted knowledge; "
            "an explanation of what changed is included instead."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"key": {"type": "string"}},
            "required": ["key"],
        },
    },
    {
        "name": "trellis_query",
        "description": (
            "Fresh structural/semantic query over the CURRENT tree (always "
            "recomputed, never cached). kind=callers|references|definitions|"
            "imports; subject is a dotted symbol path. callers/references use "
            "a labeled approximate textual resolver."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["callers", "references", "definitions", "imports"],
                },
                "subject": {"type": "string"},
            },
            "required": ["kind", "subject"],
        },
    },
    {
        "name": "trellis_publish",
        "description": (
            "Publish a reusable artifact under a key. kind=callers (key is "
            "the subject symbol), filemap (key is a module name), or notes "
            "(deps = list of module names the notes depend on)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "kind": {"type": "string", "enum": ["callers", "filemap", "notes"]},
                "value": {"type": "string"},
                "deps": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["key", "kind", "value"],
        },
    },
]


def cli_path():
    return os.environ.get("TRELLIS_BIN", "target/debug/trellis")


def run_cli(args):
    cmd = [cli_path()] + args
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if proc.returncode != 0:
        raise RuntimeError(f"trellis CLI failed ({proc.returncode}): {proc.stderr.strip()}")
    return json.loads(proc.stdout.strip())


def tool_call(name, arguments):
    base = ["--repo", os.environ["TRELLIS_REPO"], "--store", os.environ["TRELLIS_STORE"]]
    if name == "trellis_status":
        return run_cli(["status"] + base)
    if name == "trellis_retrieve":
        return run_cli(["retrieve", *base, "--key", arguments["key"]])
    if name == "trellis_query":
        return run_cli(
            [
                "query",
                "--repo",
                os.environ["TRELLIS_REPO"],
                "--kind",
                arguments["kind"],
                "--subject",
                arguments["subject"],
            ]
        )
    if name == "trellis_publish":
        args = ["publish", *base, "--key", arguments["key"], "--kind", arguments["kind"]]
        args += ["--value", arguments["value"]]
        for dep in arguments.get("deps", []):
            args += ["--dep", dep]
        return run_cli(args)
    raise RuntimeError(f"unknown tool {name}")


def handle(request):
    method = request.get("method", "")
    msg_id = request.get("id")
    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "trellis-mcp", "version": "0.2.0"},
            },
        }
    if method == "notifications/initialized":
        return None
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": msg_id, "result": {"tools": TOOLS}}
    if method == "tools/call":
        params = request.get("params", {})
        name = params.get("name", "")
        arguments = params.get("arguments", {})
        try:
            result = tool_call(name, arguments)
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [
                        {"type": "text", "text": json.dumps(result, indent=2)}
                    ],
                    "isError": False,
                },
            }
        except Exception as exc:  # surface CLI failures as tool errors
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [{"type": "text", "text": str(exc)}],
                    "isError": True,
                },
            }
    if msg_id is not None:
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"method not found: {method}"},
        }
    return None


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        response = handle(request)
        if response is not None:
            sys.stdout.write(json.dumps(response) + "\n")
            sys.stdout.flush()


def selftest():
    """Speak a full JSON-RPC handshake to a child instance of ourselves."""
    env = dict(os.environ)
    env.setdefault("TRELLIS_REPO", ".")
    env.setdefault("TRELLIS_STORE", "/tmp/trellis-mcp-selftest.db")
    proc = subprocess.Popen(
        [sys.executable, __file__],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        env=env,
    )
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    for req in requests:
        proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    responses = []
    while len(responses) < 2:
        line = proc.stdout.readline()
        if not line:
            break
        responses.append(json.loads(line))
    proc.terminate()
    assert responses[0]["result"]["serverInfo"]["name"] == "trellis-mcp", responses
    assert len(responses[1]["result"]["tools"]) == 4, responses
    print("selftest OK: initialize + tools/list round-tripped")


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        selftest()
    else:
        main()
