#!/usr/bin/env python3
"""Render angle-bracket changelog prose literally, preserving single-line code spans."""

import re
import sys

LINKS = re.compile(
    r"\[[^\]\n]*\]\([ \t]*(?P<destination><[^<>\n]*>)"
    r"|^ {0,3}(?:- )?\[[^\]\n]+\]:[ \t]*(?P<reference><[^<>\n]*>)"
    r"|(?P<autolink><(?:[A-Za-z][A-Za-z0-9+.-]{1,31}:[^<>\s]*|[^<>\s@]+@[^<>\s@]+)>)"
)


def is_escaped(text, position):
    prefix = text[:position]
    return (len(prefix) - len(prefix.rstrip("\\"))) % 2 == 1


def in_link(position, links):
    return any(start <= position < end for start, end in links)


def escape_prose(line, start, end, links):
    def replace(match):
        position = start + match.start()
        if in_link(position, links) or is_escaped(line, position):
            return match[0]
        return "&lt;" if match[0] == "<" else "&gt;"

    return re.sub(r"[<>]", replace, line[start:end])


def render_line(line):
    # Subjects are single-line. A code span closes at a run of the same number of
    # backticks; an escaped opening backtick is prose, but escapes do not apply in code.
    links = [
        match.span(match.lastgroup)
        for match in LINKS.finditer(line)
        if not is_escaped(line, match.start())
    ]
    runs = list(re.finditer(r"`+", line))
    result = []
    position = index = 0
    while index < len(runs):
        opening = runs[index]
        if in_link(opening.start(), links):
            index += 1
            continue
        start = opening.start() + int(is_escaped(line, opening.start()))
        width = opening.end() - start
        closing = next((j for j in range(index + 1, len(runs)) if len(runs[j][0]) == width), None)
        if not width or closing is None:
            index += 1
            continue
        end = runs[closing].end()
        result.extend((escape_prose(line, position, start, links), line[start:end]))
        position, index = end, closing + 1
    result.append(escape_prose(line, position, len(line), links))
    return "".join(result)


if __name__ == "__main__":
    for line in sys.stdin:
        sys.stdout.write(render_line(line))
