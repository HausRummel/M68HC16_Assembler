#!/usr/bin/env python3
"""Compare two COFF .OBJ files (e.g. golden JTE.OBJ vs ours) byte-for-byte and at
the structural level: header, section table, section data, symbol table, string
table. Ignores the 4-byte COFF timestamp (offsets 4-7), the only inherently
non-deterministic field. Built for task #2 (see the obj-handoff memory note).

Usage: objcmp.py GOLD.OBJ OURS.OBJ [--syms]
  --syms : also print per-symbol (name,val,scnum,type) mismatches and a
           type-mismatch (ours->gold) histogram.
"""
import struct
import sys


def parse(path):
    d = open(path, "rb").read()
    nsec = struct.unpack_from("<H", d, 2)[0]
    symptr = struct.unpack_from("<I", d, 8)[0]
    nsym = struct.unpack_from("<I", d, 12)[0]
    strtab = symptr + nsym * 18
    sectab = 20
    secdata = sectab + nsec * 40
    return dict(data=d, nsec=nsec, symptr=symptr, nsym=nsym, strtab=strtab,
                sectab=sectab, secdata=secdata)


def symbols(p):
    """Ordered list of program symbols (naux==0): (name, value, scnum, type)."""
    d, symptr, nsym, strtab = p["data"], p["symptr"], p["nsym"], p["strtab"]
    out = []
    i = 0
    while i < nsym:
        o = symptr + i * 18
        raw = d[o:o + 18]
        if struct.unpack_from("<I", raw, 0)[0] == 0:
            so = struct.unpack_from("<I", raw, 4)[0]
            end = d.index(b"\x00", strtab + so)
            name = d[strtab + so:end].decode("latin1")
        else:
            name = raw[0:8].split(b"\x00")[0].decode("latin1")
        val, scn, typ, _cls, na = struct.unpack_from("<IhHBB", raw, 8)
        if na == 0:
            out.append((name, val, scn, typ))
        i += 1 + na
    return out


def region(off, p):
    if off < 20:
        return "header"
    if off < p["secdata"]:
        return "sectable"
    if off < p["symptr"]:
        return "secdata"
    if off < p["strtab"]:
        return "symtab"
    return "strtab"


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    g = parse(args[0])
    o = parse(args[1])
    ga, oa = g["data"], o["data"]
    print(f"gold {len(ga)} B   ours {len(oa)} B   (sizes {'MATCH' if len(ga)==len(oa) else 'DIFFER'})")

    n = min(len(ga), len(oa))
    diffs = [i for i in range(n) if ga[i] != oa[i] and not (4 <= i < 8)]
    pct = 100 * (n - len(diffs)) / n if n else 0
    print(f"differing bytes (excl timestamp): {len(diffs)} of {n}  ({pct:.2f}% byte-exact)")
    from collections import Counter
    print("  by region:", dict(Counter(region(d, g) for d in diffs)))

    if "--syms" in sys.argv:
        gs, os_ = symbols(g), symbols(o)
        print(f"program symbols: gold {len(gs)}  ours {len(os_)}")
        order = [k for k in range(min(len(gs), len(os_))) if gs[k][0] != os_[k][0]]
        print(f"  positional order mismatches: {len(order)}"
              + (f"  first at idx {order[0]}" if order else ""))
        gt = {nm: ty for nm, _, _, ty in gs}
        ot = {nm: ty for nm, _, _, ty in os_}
        td = [(nm, ot[nm], gt[nm]) for nm in ot if nm in gt and ot[nm] != gt[nm]]
        print(f"  type mismatches (same name): {len(td)}")
        print("    (ours->gold) histogram:", dict(Counter((a, b) for _, a, b in td)))
        for a, b in sorted(set((a, b) for _, a, b in td)):
            ex = [nm for nm, x, y in td if x == a and y == b][:5]
            print(f"    {a}->{b}: {ex}")


if __name__ == "__main__":
    main()
