#!/usr/bin/env python3
"""Try candidate COFF symbol-`type` rules against the gold JTE.OBJ, instantly.

Loads the per-symbol context dump (HC16_SYMCTX) + gold types, reproduces the
current coff.rs rule (must score 482), then scores refinements so a rule can be
chosen before touching Rust. Usage: typerule.py GOLD.OBJ symctx.txt
"""
import struct
import sys
from collections import Counter


def parse(path):
    d = open(path, "rb").read()
    symptr = struct.unpack_from("<I", d, 8)[0]
    nsym = struct.unpack_from("<I", d, 12)[0]
    strtab = symptr + nsym * 18
    out = {}
    i = 0
    o = symptr
    while i < nsym:
        n8 = d[o:o + 8]
        val, scn, typ, scl, naux = struct.unpack_from("<IhHBB", d, o + 8)
        if n8[:4] == b"\x00\x00\x00\x00":
            so = strtab + struct.unpack_from("<I", n8, 4)[0]
            name = d[so:d.index(b"\x00", so)].decode("latin1")
        else:
            name = n8.split(b"\x00")[0].decode("latin1")
        if naux == 0:
            out[name] = typ
        o += 18 * (1 + naux)
        i += 1 + naux
    return out


MAP = {"W": 3, "B": 2, "R": 2, "C": 0, "-": 0}


def current(c):
    if c["kind"] == "Abs":
        return MAP[c["first"]]
    return MAP[c["first"]] if c["first"] != "-" else MAP[c["addr"]]


def main():
    gold = parse(sys.argv[1])
    ctx = {}
    for ln in open(sys.argv[2]).read().splitlines()[1:]:
        f = ln.split("\t")
        ctx[f[0]] = dict(kind=f[2], first=f[3], defe=f[4], defop=f[5],
                         addr=f[6], fdb=f[7] == "1", fcb=f[8] == "1")
    names = [n for n in gold if n in ctx]

    # R2: when a symbol is DEFINED directly on a data-directive line, its type is
    # that directive's element (overriding a forward fdb/fcb reference).
    DATA_OPS = {"fdb": 3, "dc.w": 3, "fcb": 2, "dc.b": 2, "fcc": 2, "rmb": 2, "ds": 2}

    def r2(c):
        if c["defop"] in DATA_OPS:
            return DATA_OPS[c["defop"]]
        return current(c)

    # R3: like R2, but bare/equ-defined relocatable labels take the element at
    # their address (what they mark), not a forward reference.
    def r3(c):
        if c["defop"] in DATA_OPS:
            return DATA_OPS[c["defop"]]
        if c["kind"] == "Rel" and c["defop"] in ("", "equ"):
            return MAP[c["addr"]]
        return current(c)

    rules = {"current": current, "R2_defdir": r2, "R3_defdir_addr": r3}
    for nm, r in rules.items():
        bad = sum(1 for n in names if r(ctx[n]) != gold[n])
        print(f"{nm:18s} diffs={bad}")

    # breakdown for current, to confirm 482 buckets
    hist = Counter((current(ctx[n]), gold[n]) for n in names if current(ctx[n]) != gold[n])
    print("current buckets:", dict(hist))


if __name__ == "__main__":
    main()
