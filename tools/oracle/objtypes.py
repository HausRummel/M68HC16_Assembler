#!/usr/bin/env python3
"""Compare COFF symbol `type` fields between two .OBJ files (gold vs ours).

Parses both symbol tables, aligns by name, and reports the type-mismatch
histogram (ours->gold) plus sample names per bucket — the loop for refining the
coff.rs symbol-type rule. Usage: objtypes.py GOLD.OBJ OURS.OBJ [--list ours,gold]
"""
import struct
import sys
from collections import defaultdict


def parse_syms(path):
    d = open(path, "rb").read()
    symptr = struct.unpack_from("<I", d, 8)[0]
    nsym = struct.unpack_from("<I", d, 12)[0]
    strtab = symptr + nsym * 18
    out = {}      # name -> (scnum, type)
    order = []    # names in table order (program symbols only)
    i = 0
    o = symptr
    while i < nsym:
        name8 = d[o:o + 8]
        val, scnum, typ, scl, naux = struct.unpack_from("<IhHBB", d, o + 8)
        if name8[:4] == b"\x00\x00\x00\x00":
            so = strtab + struct.unpack_from("<I", name8, 4)[0]
            end = d.index(b"\x00", so)
            name = d[so:end].decode("latin1")
        else:
            name = name8.split(b"\x00")[0].decode("latin1")
        # skip section symbols (they have aux records)
        if naux == 0:
            out[name] = (scnum, typ)
            order.append(name)
        o += 18 * (1 + naux)
        i += 1 + naux
    return out, order


def main():
    gold, _ = parse_syms(sys.argv[1])
    ours, _ = parse_syms(sys.argv[2])
    common = [n for n in gold if n in ours]
    print(f"gold syms={len(gold)} ours syms={len(ours)} common={len(common)}")

    type_hist = defaultdict(list)   # (ours_type, gold_type) -> [names]
    scnum_diff = []
    for n in common:
        gs, gt = gold[n]
        os_, ot = ours[n]
        if gs != os_:
            scnum_diff.append((n, os_, gs))
        if gt != ot:
            type_hist[(ot, gt)].append(n)

    only_gold = [n for n in gold if n not in ours]
    only_ours = [n for n in ours if n not in gold]
    print(f"missing-from-ours={len(only_gold)} extra-in-ours={len(only_ours)}")
    print(f"scnum diffs={len(scnum_diff)}")
    total_t = sum(len(v) for v in type_hist.values())
    print(f"\nTYPE mismatches (ours->gold) total={total_t}:")
    for k in sorted(type_hist, key=lambda k: -len(type_hist[k])):
        names = type_hist[k]
        print(f"  {k[0]}->{k[1]} = {len(names):4d}   e.g. {' '.join(names[:6])}")

    if len(sys.argv) > 3 and sys.argv[3].startswith("--list"):
        ot, gt = (int(x) for x in sys.argv[4].split(","))
        print(f"\nall names with ours={ot} gold={gt}:")
        for n in type_hist[(ot, gt)]:
            print(" ", n)


if __name__ == "__main__":
    main()
