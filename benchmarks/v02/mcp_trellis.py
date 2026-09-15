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
        "name": "code_query",
        "description": (
            "Structural query over the repository. kind: 'callers' = modules "
            "that call the symbol (dotted path like auth.tokens.refresh_token); "
            "'definitions' = definitions in a module (dotted module name); "
            "'imports' = import list of a module (dotted module name). Returns "
            "a semicolon-joined value string. Use this for structural "
            "questions instead of grepping by hand."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["callers", "references", "definitions", "imports"]},
                "subject": {"type": "string"},
            },
            "required": ["kind", "subject"],
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
    if name != "code_query":
        raise RuntimeError(f"unknown tool {name}")
    args = [
        "tool",
        "--repo", os.environ["TRELLIS_REPO"],
        "--mode", os.environ.get("TRELLIS_MODE", "baseline"),
        "--kind", arguments["kind"],
        "--subject", arguments["subject"],
    ]
    if os.environ.get("TRELLIS_MODE", "baseline") == "trellis":
        args += ["--store", os.environ["TRELLIS_STORE"]]
    return run_cli(args)


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
    assert len(responses[1]["result"]["tools"]) == 1, responses
    print("selftest OK: initialize + tools/list round-tripped")


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        selftest()
    else:
        main()
