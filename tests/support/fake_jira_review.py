"""A stand-in for `jira-review mcp`: MCP over stdin and stdout, one JSON-RPC
message per line, answering `search` pages from a fixed Jira.

Its own options come first, then what Workboard passed:

    python3 fake_jira_review.py --log FILE --mode MODE [jira-review args...]

FILE gets one JSON line for the argv and one for each message received.
MODE is `pages` (two pages joined by a cursor), `tool_error` (every search
reports an error, as jira-review does without a credential) or `exit` (it
fails at start, as a bad flag would).
"""

import json
import sys

args = sys.argv[1:]
log_path = args[args.index("--log") + 1]
mode = args[args.index("--mode") + 1]
passed = args[args.index("--mode") + 2 :]


def log(entry):
    with open(log_path, "a") as f:
        f.write(json.dumps(entry) + "\n")


log({"argv": passed})
if mode == "exit":
    sys.stderr.write("error: unexpected argument '--bogus' found\n")
    sys.exit(2)

SITE = "https://example.atlassian.net"


def issue(id, key, category):
    return {
        "id": id, "key": key, "summary": "A summary", "status": "In Progress",
        "status_category": category, "priority": "Planned", "issue_type": "Task",
        "labels": ["phase-3"],
        "parent": {"key": "ABC-1", "summary": "The epic", "issue_type": "Epic"},
    }


PAGES = {
    None: {"site": SITE, "account_email": "user@example.com",
           "issues": [issue("1", "ABC-2", "new")], "next_cursor": "next-page"},
    "next-page": {"site": SITE, "account_email": "user@example.com",
                  "issues": [issue("2", "ABC-3", "done")], "next_cursor": None},
}

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    log(msg)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion": msg["params"]["protocolVersion"],
                  "capabilities": {"tools": {"listChanged": False}},
                  "serverInfo": {"name": "jira-review", "version": "0.0.0"}}
    elif method == "tools/call" and msg["params"]["name"] == "search":
        if mode == "tool_error":
            result = {"content": [{"type": "text", "text": "no Jira credential: set JIRA_URL"}],
                      "isError": True}
        else:
            page = PAGES[msg["params"]["arguments"].get("cursor")]
            result = {"content": [{"type": "text", "text": json.dumps(page)}], "isError": False}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"],
                          "error": {"code": -32601, "message": f"no method {method}"}}),
              flush=True)
        continue
    # A notification first, which a client must skip.
    print(json.dumps({"jsonrpc": "2.0", "method": "notifications/message", "params": {}}),
          flush=True)
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
