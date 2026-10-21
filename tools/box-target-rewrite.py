#!/usr/bin/env python3
"""Rewrite `box_name` to `target` in struct literals of the requests that take a BoxTarget.

Usage: tools/box-target-rewrite.py [paths...]   (default: crates)

Rewrites, at the literal's top level only:
  box_name: E          -> target: E.into()
  box_name             -> target: box_name.into()
  ShowReq box_name: None     -> target: None
  ShowReq box_name: Some(E)  -> target: Some(E.into())
  ShowReq box_name: E        -> target: E.map(Into::into)
Prints each rewritten file. Rerunning on rewritten code changes nothing.
"""

import pathlib
import re
import sys

TYPES = ("SteerReq", "CloseReq", "ShowReq", "ArtifactsReq", "ArtifactReadReq")
START = re.compile(r"\b(" + "|".join(TYPES) + r")\s*\{")


def close_of(text, i):
    """Index of the brace matching the one at text[i]; skips string literals."""
    depth = 0
    j = i
    while j < len(text):
        c = text[j]
        if c == '"':
            j += 1
            while text[j] != '"':
                j += 2 if text[j] == "\\" else 1
        elif c in "{([":
            depth += 1
        elif c in "})]":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    raise ValueError("unbalanced literal")


def fields(body):
    """Split a literal body at top-level commas into (start, end) spans."""
    spans, depth, start, j = [], 0, 0, 0
    while j < len(body):
        c = body[j]
        if c == '"':
            j += 1
            while body[j] != '"':
                j += 2 if body[j] == "\\" else 1
        elif c in "{([":
            depth += 1
        elif c in "})]":
            depth -= 1
        elif c == "," and depth == 0:
            spans.append((start, j))
            start = j + 1
        j += 1
    spans.append((start, len(body)))
    return spans


def is_pattern(text, end, body):
    """A destructuring pattern, not a literal: it has a `..` rest or is followed by `=`, `=>` or `|`."""
    if any(body[a:b].strip() == ".." for a, b in fields(body)):
        return True
    after = text[end + 1 :].lstrip(") \n\t")
    return after.startswith(("=>", "|")) or (after.startswith("=") and not after.startswith("=="))


def wrap(expr):
    """Parenthesize `expr` unless a method call binds to all of it."""
    top, depth, j = [], 0, 0
    while j < len(expr):
        c = expr[j]
        if c == '"':
            j += 1
            while expr[j] != '"':
                j += 2 if expr[j] == "\\" else 1
        elif c in "{([":
            depth += 1
        elif c in "})]":
            depth -= 1
        elif depth == 0:
            top.append(c)
        j += 1
    return f"({expr})" if any(c in " &*!-+|=/%^" for c in top) else expr


def rewrite_field(kind, field):
    lead = field[: len(field) - len(field.lstrip())]
    trail = field[len(field.rstrip()) :]
    core = field.strip()
    if core == "box_name":
        core = "box_name: box_name"
    if not core.startswith("box_name:"):
        return None
    expr = core[len("box_name:") :].strip()
    if kind != "ShowReq":
        return f"{lead}target: {wrap(expr)}.into(){trail}"
    if expr == "None":
        return f"{lead}target: None{trail}"
    some = re.fullmatch(r"Some\((.*)\)", expr, re.S)
    if some:
        return f"{lead}target: Some({wrap(some.group(1).strip())}.into()){trail}"
    return f"{lead}target: {wrap(expr)}.map(Into::into){trail}"


def opens_block(text, at):
    """The brace at `at` opens a struct, impl or fn body, not a literal."""
    return text[:at].rstrip().endswith(("struct", "->"))


def literals(text):
    """(open brace index, type) of each literal; `Self { .. }` counts inside `impl .. for TYPE`."""
    for m in START.finditer(text):
        open_at = m.end() - 1
        if opens_block(text, m.start()):
            continue
        if text[: m.start()].rstrip().endswith("for"):
            end = close_of(text, open_at)
            for s in re.finditer(r"\bSelf\s*\{", text[open_at:end]):
                if not opens_block(text, open_at + s.start()):
                    yield open_at + s.end() - 1, m.group(1)
            continue
        yield open_at, m.group(1)


def rewrite(text):
    for open_at, kind in sorted(literals(text), reverse=True):
        end = close_of(text, open_at)
        body = text[open_at + 1 : end]
        if is_pattern(text, end, body):
            continue
        parts = []
        for a, b in fields(body):
            new = rewrite_field(kind, body[a:b])
            parts.append(body[a:b] if new is None else new)
        text = text[: open_at + 1] + ",".join(parts) + text[end:]
    return text


def main():
    roots = [pathlib.Path(p) for p in (sys.argv[1:] or ["crates"])]
    for root in roots:
        for path in sorted(root.rglob("*.rs") if root.is_dir() else [root]):
            text = path.read_text()
            new = rewrite(text)
            if new != text:
                path.write_text(new)
                print(path)


if __name__ == "__main__":
    main()
