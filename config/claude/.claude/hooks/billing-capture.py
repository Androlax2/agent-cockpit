#!/usr/bin/env python3
"""Capture reply funding without credentials or access to the host usage archive."""

import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile

MAX_RECORD_BYTES = 4096


def open_regular(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    if not stat.S_ISREG(os.fstat(descriptor).st_mode):
        os.close(descriptor)
        raise ValueError("billing input must be a regular file")
    return os.fdopen(descriptor, "rb")


def save(record):
    if not isinstance(record, dict) or set(record) != {"key", "billing"}:
        raise ValueError("billing metadata must contain only key and billing")
    key = record["key"]
    if not isinstance(key, str) or not re.fullmatch(r"[A-Za-z0-9:_-]{1,512}", key):
        raise ValueError("invalid billing reply key")
    if record["billing"] not in ("api", "subscription", "unknown"):
        raise ValueError("invalid billing mode")
    payload = json.dumps({"key": key, "billing": record["billing"]}, separators=(",", ":")).encode()
    directory = Path.home() / ".cache/claude/billing"
    directory.mkdir(parents=True, exist_ok=True)
    if directory.is_symlink():
        raise ValueError("billing directory must not be a symlink")
    destination = directory / (hashlib.sha1(payload).hexdigest() + ".json")
    # Immutable, mode-specific records retain evidence when captures disagree or race.
    descriptor, temporary = tempfile.mkstemp(prefix=".", suffix=".tmp", dir=directory)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
        try:
            os.link(temporary, destination)
        except FileExistsError:
            with open_regular(destination) as existing:
                if existing.read(MAX_RECORD_BYTES + 1) != payload:
                    raise ValueError("billing record conflicts with its filename")
    finally:
        os.unlink(temporary)


def timestamp(value):
    parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("billing timestamp must include a timezone")
    return parsed


def capture_lines(transcript, mode, started):
    offset = transcript.tell()
    while line := transcript.readline():
        if line.endswith(b"\n"):
            offset = transcript.tell()
        if b'"usage"' not in line:
            continue
        try:
            record = json.loads(line)
            message = record.get("message", {})
            model = message.get("model", "")
            if not isinstance(message.get("usage"), dict) or not isinstance(model, str) or not model or model.startswith("<"):
                continue
            if timestamp(record["timestamp"]) < started:
                continue
            message_id = message.get("id") if isinstance(message.get("id"), str) else ""
            request_id = record.get("requestId") if isinstance(record.get("requestId"), str) else ""
            key = message_id + ":" + request_id if message_id or request_id else "line:" + hashlib.sha1(line).hexdigest()
        except (ValueError, TypeError, KeyError, AttributeError):
            continue
        save({"key": key, "billing": mode})
    return offset


def boundary_digest(transcript, offset):
    # shortcut: append-only transcripts; arbitrary edits deep in an old prefix require cursor removal.
    transcript.seek(0)
    prefix = transcript.read(min(offset, 256))
    transcript.seek(max(0, offset - 256))
    return hashlib.sha256(prefix + transcript.read(min(offset, 256))).hexdigest()


def open_cursor(path):
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        descriptor = os.open(path.name, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK,
                             0o600, dir_fd=directory)
    finally:
        os.close(directory)
    return os.fdopen(descriptor, "r+b")


def capture_transcript(mode, since, path):
    if mode not in ("api", "subscription"):
        raise ValueError("transcript billing must be api or subscription")
    started = timestamp(since)
    directory = Path.home() / ".cache/claude/billing-cursors"
    directory.mkdir(parents=True, exist_ok=True)
    identity = json.dumps([os.path.abspath(path), mode, since]).encode()
    cursor_path = directory / (hashlib.sha256(identity).hexdigest() + ".json")
    with open_cursor(cursor_path) as cursor:
        metadata = os.fstat(cursor.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
            raise ValueError("billing cursor must be a regular file with one link")
        fcntl.flock(cursor, fcntl.LOCK_EX)
        try:
            cached = json.loads(cursor.read(MAX_RECORD_BYTES + 1))
        except (ValueError, UnicodeError):
            cached = None
        with open_regular(path) as transcript:
            metadata = os.fstat(transcript.fileno())
            offset = cached.get("offset") if isinstance(cached, dict) else None
            if not (type(offset) is int and 0 <= offset <= metadata.st_size
                    and cached.get("device") == metadata.st_dev and cached.get("inode") == metadata.st_ino
                    and not (cached.get("size") == metadata.st_size and cached.get("mtime_ns") != metadata.st_mtime_ns)
                    and cached.get("boundary") == boundary_digest(transcript, offset)):
                offset = 0
            transcript.seek(offset)
            offset = capture_lines(transcript, mode, started)
            state = {"device": metadata.st_dev, "inode": metadata.st_ino, "offset": offset,
                     "size": metadata.st_size, "mtime_ns": metadata.st_mtime_ns,
                     "boundary": boundary_digest(transcript, offset)}
        cursor.seek(0)
        cursor.write(json.dumps(state, separators=(",", ":")).encode())
        cursor.truncate()


def main():
    arguments = sys.argv[1:]
    if arguments == ["record-billing"]:
        payload = sys.stdin.buffer.read(MAX_RECORD_BYTES + 1)
        if len(payload) > MAX_RECORD_BYTES:
            raise ValueError("billing record exceeds 4096 bytes")
        save(json.loads(payload))
    elif len(arguments) == 4 and arguments[0] == "record-transcript-billing":
        capture_transcript(*arguments[1:])
    else:
        raise ValueError("expected record-billing or record-transcript-billing <mode> <since> <path>")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError) as error:
        print(f"billing capture: {error}", file=sys.stderr)
        sys.exit(1)
