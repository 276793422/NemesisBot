#!/usr/bin/env python3
"""Add msg_class to struct-literal InboundMessage initializers in test files.

For every `InboundMessage {` struct literal (nemesis_types::channel:: path or
bare) lacking a `msg_class` field, insert
`msg_class: nemesis_types::channel::MsgClass::User,` right before the closing
brace of that literal. Idempotent: skips literals that already have the field.
"""
import re
import sys


def fix_file(path: str) -> int:
    with open(path, encoding="utf-8") as f:
        src = f.read()
    out = []
    pos = 0
    count = 0
    # struct-literal starts: `InboundMessage {` possibly qualified
    for m in re.finditer(r"(?:nemesis_types::channel::|channel::)?InboundMessage\s*\{", src):
        start = m.end()  # just after `{`
        # find matching closing brace (braces don't nest inside these
        # literals except in metadata hash init like HashMap::new() or
        # [...].into_iter().collect() — no brace literals; but be safe and
        # track depth)
        depth = 1
        i = start
        while i < len(src) and depth > 0:
            c = src[i]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
            i += 1
        if depth != 0:
            continue
        end = i - 1  # index of closing `}`
        body = src[start:end]
        if "msg_class" in body:
            continue
        # determine indentation from the first field line after `{`
        nl = src.find("\n", start)
        if nl == -1 or nl > end:
            # single-line literal: append before `}`
            out.append(src[pos:end])
            out.append(" msg_class: nemesis_types::channel::MsgClass::User")
            pos = end
            count += 1
            continue
        line = src[start:nl]
        indent = line[: len(line) - len(line.lstrip())]
        out.append(src[pos:end])
        out.append(f"{indent}msg_class: nemesis_types::channel::MsgClass::User,\n")
        pos = end
        count += 1
    if count:
        out.append(src[pos:])
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write("".join(out))
    return count


def main() -> None:
    for path in sys.argv[1:]:
        n = fix_file(path)
        print(f"{path}: {n} fixed")


if __name__ == "__main__":
    main()
