#!/usr/bin/env python3
"""A pytest junit report, summed up in Markdown: the counts, then every
test that did not pass, with the first line of its message.

    summary.py JUNIT >> "$GITHUB_STEP_SUMMARY"

The rest (tracebacks, celily's logs) is in the report. Standard library
only, like driver.py.
"""

import argparse
import collections
import re
import xml.etree.ElementTree as ET

# In this order, in the counts and the table.
OUTCOMES = ["failed", "error", "xfailed", "skipped", "passed"]
MESSAGE_MAX = 200


def outcome(case):
    """The outcome of a <testcase> and its message. A test failing, then
    erroring in teardown, has two <testcase>."""
    for child in case:
        match child.tag, child.get("type"):
            case "failure", _:
                return "failed", child.get("message", "")
            case "error", _:
                return "error", child.get("message", "")
            case "skipped", "pytest.xfail":
                return "xfailed", child.get("message", "")
            case "skipped", _:
                return "skipped", child.get("message", "")
    return "passed", ""


def inline(text):
    """`text` in a table cell: every ASCII punctuation character escaped
    (`|` ends the cell, `<` opens HTML)."""
    return re.sub(r"([!-/:-@\[-`{-~])", r"\\\1", text)


def summary(junit):
    rows = []
    for case in ET.parse(junit).iter("testcase"):
        result, message = outcome(case)
        lines = message.splitlines() or [""]
        if len(lines) > 1 or len(lines[0]) > MESSAGE_MAX:
            lines[0] = lines[0][:MESSAGE_MAX] + "..."
        test = f"{case.get('classname')}.{case.get('name')}"
        rows.append((result, test, lines[0]))
    counts = collections.Counter(result for result, _, _ in rows)
    # As pytest says it: "1 error", "2 errors".
    words = [
        f"{n} {o}s" if o == "error" and n > 1 else f"{n} {o}"
        for o in OUTCOMES
        if (n := counts[o])
    ]
    out = [", ".join(words) or "No tests."]
    listed = sorted(
        (row for row in rows if row[0] != "passed"),
        key=lambda row: OUTCOMES.index(row[0]),
    )
    if listed:
        out += ["", "| Test | Outcome | Message |", "| --- | --- | --- |"]
        out += [f"| {inline(t)} | {r} | {inline(m)} |" for r, t, m in listed]
    return "\n".join(out)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("junit", help="the report, from pytest --junitxml")
    print(summary(parser.parse_args().junit))


if __name__ == "__main__":
    main()
