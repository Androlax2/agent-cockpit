#!/usr/bin/env python3
"""Read bounded visible native Codex history through its local read-only RPCs."""

import json
from pathlib import Path
import re
import runpy
import signal
import sys

protocol = runpy.run_path(str(Path(__file__).with_name("codex-quota.py")))
QuotaError = protocol["QuotaError"]
TEXT_MAX = 256 * 1024
SOURCE_MAX = 4 * 1024 * 1024
MESSAGES_MAX = 100


def read_session(native):
    with protocol["app_server"](execution=True) as process:
        protocol["request"](process, 1, "initialize", {"clientInfo": {"name": "cockpit_history", "version": "1"}})
        protocol["send"](process, {"method": "initialized", "params": {}})
        thread = protocol["request"](process, 2, "thread/read", {"threadId": native, "includeTurns": False}).get("thread")
        if not isinstance(thread, dict) or thread.get("id") != native:
            raise QuotaError("Codex thread identity does not match the requested conversation")
        history_mode = thread.get("historyMode", "legacy")
        if history_mode not in ("legacy", "paginated"):
            raise QuotaError("Codex conversation history mode is unavailable")
        legacy_entries = None
        if history_mode == "legacy":
            legacy = protocol["request"](process, 3, "thread/read", {"threadId": native, "includeTurns": True}).get("thread")
            if not isinstance(legacy, dict) or legacy.get("id") != native:
                raise QuotaError("Codex thread identity does not match the requested conversation")
            turns = legacy.get("turns")
            if not isinstance(turns, list) or any(not isinstance(turn, dict) or not isinstance(turn.get("items"), list) for turn in turns):
                raise QuotaError("Codex conversation history format is unavailable")
            legacy_entries = [{"item": item} for turn in reversed(turns) for item in reversed(turn["items"])]
        messages = []
        latest_user = None
        consumed = 0
        cursor = None
        seen_cursors = set()
        truncated = False
        for page in range(100):
            parameters = {"threadId": native, "limit": 20, "sortDirection": "desc"}
            if cursor is not None:
                parameters["cursor"] = cursor
            result = {"data": legacy_entries} if legacy_entries is not None else protocol["request"](process, page + 3, "thread/items/list", parameters)
            consumed += len(json.dumps(result).encode())
            if consumed > SOURCE_MAX:
                truncated = True
                break
            entries = result.get("data")
            if not isinstance(entries, list):
                raise QuotaError("Codex conversation history format is unavailable")
            for entry in entries:
                item = entry.get("item") if isinstance(entry, dict) else None
                if not isinstance(item, dict):
                    raise QuotaError("Codex conversation contains an invalid item")
                kind = item.get("type")
                if kind == "agentMessage":
                    role, text = "assistant", item.get("text")
                elif kind == "userMessage":
                    content = item.get("content")
                    if not isinstance(content, list):
                        raise QuotaError("Codex conversation contains an invalid user message")
                    text = "\n\n".join(block["text"] for block in content
                        if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("text"), str))
                    role = "user"
                else:
                    continue
                if not isinstance(text, str):
                    raise QuotaError("Codex conversation contains invalid visible text")
                if not text.strip():
                    continue
                if role == "user" and latest_user is None:
                    if len(text.encode()) > TEXT_MAX:
                        raise QuotaError("Latest Codex request exceeds the safe handoff size")
                    latest_user = text
                if len(messages) < MESSAGES_MAX:
                    identity = item.get("id")
                    if not isinstance(identity, str):
                        raise QuotaError("Codex conversation item has no identity")
                    messages.append({"id": identity, "role": role, "text": text})
                else:
                    truncated = True
            cursor = result.get("nextCursor")
            if cursor is None:
                break
            if not isinstance(cursor, str) or not cursor or cursor in seen_cursors:
                raise QuotaError("Codex conversation pagination is invalid")
            seen_cursors.add(cursor)
            if len(messages) >= MESSAGES_MAX and latest_user is not None:
                truncated = True
                break
        else:
            truncated = True
    messages.reverse()
    size = sum(len(message["text"].encode()) for message in messages)
    while size > TEXT_MAX and len(messages) > 1:
        size -= len(messages.pop(0)["text"].encode())
        truncated = True
    if size > TEXT_MAX:
        messages[0]["text"] = messages[0]["text"].encode()[-TEXT_MAX:].decode("utf-8", errors="ignore")
        truncated = True
    return {"thread": native, "model": thread.get("model"), "provider": thread.get("modelProvider"),
            "effort": thread.get("reasoningEffort"), "messages": messages, "latest_user": latest_user, "truncated": truncated}


def main():
    if len(sys.argv) != 2 or re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", sys.argv[1]) is None:
        print("invalid native thread", file=sys.stderr)
        return 1
    signal.signal(signal.SIGALRM, protocol["timeout_handler"])
    signal.signal(signal.SIGTERM, protocol["cancellation_handler"])
    signal.setitimer(signal.ITIMER_REAL, 15)
    try:
        print(json.dumps(read_session(sys.argv[1]), allow_nan=False))
        return 0
    except QuotaError as error:
        print(str(error), file=sys.stderr)
    except (OSError, ValueError, TypeError, KeyError):
        print("Codex conversation history is unavailable", file=sys.stderr)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    return 1


if __name__ == "__main__":
    sys.exit(main())
