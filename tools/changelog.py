#!/usr/bin/env python3
"""Render angle-bracket changelog prose literally, preserving single-line code spans."""

import re
import sys


def escape_prose(text):
    return text.replace("<", "&lt;").replace(">", "&gt;")


def render_line(line):
    # Subjects are single-line. A code span closes at a run of the same number of
    # backticks; an escaped opening backtick is prose, but escapes do not apply in code.
    runs = list(re.finditer(r"`+", line))
    result = []
    position = index = 0
    while index < len(runs):
        opening = runs[index]
        prefix = line[: opening.start()]
        escaped = (len(prefix) - len(prefix.rstrip("\\"))) % 2
        closing = next((j for j in range(index + 1, len(runs)) if runs[j][0] == opening[0]), None)
        if escaped or closing is None:
            index += 1
            continue
        end = runs[closing].end()
        result.extend((escape_prose(line[position : opening.start()]), line[opening.start() : end]))
        position, index = end, closing + 1
    result.append(escape_prose(line[position:]))
    return "".join(result)


if __name__ == "__main__":
    for line in sys.stdin:
        sys.stdout.write(render_line(line))
