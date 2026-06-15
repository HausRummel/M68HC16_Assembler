#!/usr/bin/env python
"""Diff our .LST body against the golden MASM listing.

Usage: lstbody.py GOLD.LST OUR_BODY.txt [--max N] [--around ABS]

The golden listing is paginated: every page is preceded by a 6-line header block
(the "Motorola Macro Assembler" banner, the file/timestamp line, the TTL title, a
blank line, and the two column-header rows). We strip those blocks and the trailing
Symbol Table / Cross-Reference sections, leaving the pure body to compare against
our renderer's continuous output.
"""
import sys

HEADER_TRIGGER = "Motorola Macro Assembler"
HEADER_LINES = 6  # banner + file line + title + blank + "Abs." + dashes


def gold_body(path):
    rows = []
    skip = 0
    with open(path, "r", encoding="latin-1", newline="") as f:
        for raw in f:
            line = raw.rstrip("\n")
            # Stop at the Symbol Table (end of body).
            if line.startswith("Symbol Table:"):
                break
            if skip > 0:
                skip -= 1
                continue
            if HEADER_TRIGGER in line:
                skip = HEADER_LINES - 1
                continue
            rows.append(line)
    return rows


def our_body(path):
    with open(path, "r", encoding="latin-1", newline="") as f:
        return [l.rstrip("\n") for l in f]


def main():
    args = sys.argv[1:]
    max_show = 30
    around = None
    pos = []
    i = 0
    while i < len(args):
        if args[i] == "--max":
            max_show = int(args[i + 1]); i += 2
        elif args[i] == "--around":
            around = int(args[i + 1]); i += 2
        else:
            pos.append(args[i]); i += 1
    gold = gold_body(pos[0])
    ours = our_body(pos[1])
    print(f"gold body rows = {len(gold)}")
    print(f"our  body rows = {len(ours)}")

    n = min(len(gold), len(ours))
    mismatches = []
    for k in range(n):
        if gold[k] != ours[k]:
            mismatches.append(k)
    print(f"common prefix = {n}; mismatches = {len(mismatches)}"
          f" ({100.0*len(mismatches)/n:.3f}%)" if n else "")

    if around is not None:
        # show rows whose Abs == around
        for k in range(n):
            if gold[k][:4].strip() == str(around) or ours[k][:4].strip() == str(around):
                print(f"[{k}] G: {gold[k]!r}")
                print(f"     O: {ours[k]!r}")
        return

    shown = 0
    for k in mismatches:
        if shown >= max_show:
            break
        print(f"--- mismatch at body row {k} (gold Abs={gold[k][:4].strip()}) ---")
        print(f"  G: {gold[k]!r}")
        print(f"  O: {ours[k]!r}")
        shown += 1
    if len(gold) != len(ours):
        print(f"\nLENGTH DIFF: gold={len(gold)} ours={len(ours)}")
        tail = min(len(gold), len(ours))
        if tail < len(gold):
            print("first gold rows past common length:")
            for r in gold[tail:tail+5]:
                print("  G:", repr(r))
        if tail < len(ours):
            print("first our rows past common length:")
            for r in ours[tail:tail+5]:
                print("  O:", repr(r))


if __name__ == "__main__":
    main()
