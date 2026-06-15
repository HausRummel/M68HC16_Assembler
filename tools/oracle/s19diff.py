#!/usr/bin/env python3
"""Diff two S-record files as address->byte memory images.

Usage: s19diff.py GOLD.S19 OURS.S19 [--limit N]

Reports: byte count in each, address span, count of addresses present in one
but not the other, count of addresses present in both that differ, and the first
N differing addresses (with a little surrounding context).
"""
import sys


def parse_s19(path):
    mem = {}
    with open(path, "r", errors="replace") as f:
        for line in f:
            line = line.strip()
            if len(line) < 4 or line[0] != "S":
                continue
            t = line[1]
            if t not in "123":
                continue  # skip S0 header, S5 count, S7/8/9 termination
            alen = {"1": 2, "2": 3, "3": 4}[t]
            count = int(line[2:4], 16)
            addr = int(line[4:4 + alen * 2], 16)
            # count = address bytes + data bytes + 1 checksum byte. Data ends one
            # byte (two hex chars) before the end of the count-covered payload.
            data_hex = line[4 + alen * 2: 4 + (count - 1) * 2]
            data = bytes.fromhex(data_hex)
            for i, b in enumerate(data):
                mem[addr + i] = b
    return mem


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    limit = 40
    for a in sys.argv[1:]:
        if a.startswith("--limit"):
            limit = int(a.split("=")[1]) if "=" in a else 40
    gold = parse_s19(args[0])
    ours = parse_s19(args[1])

    gk = set(gold)
    ok = set(ours)
    only_gold = gk - ok
    only_ours = ok - gk
    both = gk & ok
    diff = sorted(a for a in both if gold[a] != ours[a])

    print(f"gold bytes={len(gold)}  span={min(gk):#06x}..{max(gk):#06x}")
    print(f"ours bytes={len(ours)}  span={min(ok):#06x}..{max(ok):#06x}")
    print(f"only in gold: {len(only_gold)}   only in ours: {len(only_ours)}")
    print(f"common addrs: {len(both)}   differing: {len(diff)}  "
          f"({100*len(diff)/max(1,len(both)):.1f}%)")
    if only_gold:
        s = sorted(only_gold)
        print(f"  first gold-only addrs: {[hex(a) for a in s[:8]]}")
    if only_ours:
        s = sorted(only_ours)
        print(f"  first ours-only addrs: {[hex(a) for a in s[:8]]}")
    # Coalesce differing common addresses into contiguous runs.
    runs = []
    for a in diff:
        if runs and a == runs[-1][1] + 1:
            runs[-1][1] = a
        else:
            runs.append([a, a])
    print(f"differing runs: {len(runs)}")
    print(f"first {limit} runs [start..end] (len):")
    for r in runs[:limit]:
        ln = r[1] - r[0] + 1
        print(f"  {r[0]:#07x}..{r[1]:#07x}  len={ln}")
    # Largest runs:
    big = sorted(runs, key=lambda r: r[1] - r[0], reverse=True)[:10]
    print("largest runs:")
    for r in big:
        print(f"  {r[0]:#07x}..{r[1]:#07x}  len={r[1]-r[0]+1}")


if __name__ == "__main__":
    main()
