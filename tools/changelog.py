#!/usr/bin/env python3
"""Render angle-bracket changelog prose literally, preserving single-line code spans."""

import re
import sys

ANGLES = re.compile(
    r"(?P<link>(?:\]\([ \t]*|\]:[ \t]*)<[^<>\n]*>"
    r"|<(?:[A-Za-z][A-Za-z0-9+.-]{1,31}:[^<>\s]*|[^<>\s@]+@[^<>\s@]+)>)|[<>]"
)


def is_escaped(text, position):
    prefix = text[:position]
    return (len(prefix) - len(prefix.rstrip("\\"))) % 2 == 1


def escape_prose(text):
    def replace(match):
        if match["link"] or is_escaped(text, match.start()):
            return match[0]
        return "&lt;" if match[0] == "<" else "&gt;"

    return ANGLES.sub(replace, text)


def render_line(line):
    # Subjects are single-line. A code span closes at a run of the same number of
    # backticks; an escaped opening backtick is prose, but escapes do not apply in code.
    runs = list(re.finditer(r"`+", line))
    result = []
    position = index = 0
    while index < len(runs):
        opening = runs[index]
        start = opening.start() + int(is_escaped(line, opening.start()))
        width = opening.end() - start
        closing = next((j for j in range(index + 1, len(runs)) if len(runs[j][0]) == width), None)
        if not width or closing is None:
            index += 1
            continue
        end = runs[closing].end()
        result.extend((escape_prose(line[position:start]), line[start:end]))
        position, index = end, closing + 1
    result.append(escape_prose(line[position:]))
    return "".join(result)


if __name__ == "__main__":
    for line in sys.stdin:
        sys.stdout.write(render_line(line))
