#!/usr/bin/env python3
"""Full-file .LST compare (body + Symbol Table + Cross-Reference), modulo the two
inherent/input-dependent fields: the per-page wall-clock timestamp and the top
file name in header line 2 (`68HC16 - <file> - <timestamp>`). MASM's oracle renames
the input to IN.ASM; our CLI keeps the real basename, so both are normalised away.

Usage: lstfull.py GOLD.LST OURS.LST [--max N]
"""
import sys


def norm(line):
    # Collapse the whole "68HC16 - <file> - <timestamp>" header line.
    if line.startswith("68HC16 - "):
        return "68HC16 -"
    return line


def rd(path):
    with open(path, "r", encoding="latin-1", newline="") as f:
        return [l.rstrip("\n") for l in f]


def main():
    args = [a for a in sys.argv[1:] if a != "--max"]
    max_show = 25
    if "--max" in sys.argv:
        max_show = int(sys.argv[sys.argv.index("--max") + 1])
    gold, ours = rd(args[0]), rd(args[1])
    print(f"gold lines = {len(gold)}   ours lines = {len(ours)}")
    n = min(len(gold), len(ours))
    mism = [k for k in range(n) if norm(gold[k]) != norm(ours[k])]
    print(f"common = {n}   mismatches (modulo timestamp+top-name) = {len(mism)}"
          + (f"  ({100.0*len(mism)/n:.4f}%)" if n else ""))
    shown = 0
    for k in mism:
        if shown >= max_show:
            break
        print(f"-- line {k+1} --")
        print(f"  G: {gold[k]!r}")
        print(f"  O: {ours[k]!r}")
        shown += 1
    if len(gold) != len(ours):
        print(f"\nLENGTH DIFF: gold={len(gold)} ours={len(ours)}")


if __name__ == "__main__":
    main()
