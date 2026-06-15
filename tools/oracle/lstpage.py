#!/usr/bin/env python
"""Diff our paginated .LST body against the golden MASM listing, modulo the
per-page timestamp (which MASM stamps from wall-clock, so it is non-deterministic).

Usage: lstpage.py GOLD.LST OUR_PAGE.txt [--max N]

The golden body section runs from the top of the file through the "N lines
assembled" tally (the Symbol Table follows on the next page). Header line 2
(`68HC16 - FILE - TIMESTAMP`) has its timestamp normalised away on both sides.
"""
import re
import sys

TS = re.compile(r"^(68HC16 - .*? - ).*$")


def normalize(line):
    m = TS.match(line)
    return m.group(1) if m else line


def gold_body(path):
    rows = []
    with open(path, "r", encoding="latin-1", newline="") as f:
        for raw in f:
            line = raw.rstrip("\n")
            rows.append(line)
            if re.match(r"^\d+ lines assembled\r?$", line):
                break
    return rows


def read_rows(path):
    with open(path, "r", encoding="latin-1", newline="") as f:
        return [l.rstrip("\n") for l in f]


def main():
    args = sys.argv[1:]
    max_show = 30
    pos = []
    i = 0
    while i < len(args):
        if args[i] == "--max":
            max_show = int(args[i + 1]); i += 2
        else:
            pos.append(args[i]); i += 1

    gold = gold_body(pos[0])
    ours = read_rows(pos[1])
    print(f"gold body lines = {len(gold)}")
    print(f"our  body lines = {len(ours)}")

    n = min(len(gold), len(ours))
    mism = [k for k in range(n) if normalize(gold[k]) != normalize(ours[k])]
    print(f"common prefix = {n}; mismatches (modulo timestamp) = {len(mism)}"
          + (f" ({100.0*len(mism)/n:.4f}%)" if n else ""))

    shown = 0
    for k in mism:
        if shown >= max_show:
            break
        print(f"--- mismatch at line {k} ---")
        print(f"  G: {gold[k]!r}")
        print(f"  O: {ours[k]!r}")
        shown += 1
    if len(gold) != len(ours):
        print(f"\nLENGTH DIFF: gold={len(gold)} ours={len(ours)}")
        tail = min(len(gold), len(ours))
        for r in gold[tail:tail + 4]:
            print("  G>", repr(r))
        for r in ours[tail:tail + 4]:
            print("  O>", repr(r))


if __name__ == "__main__":
    main()
