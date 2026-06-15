#!/usr/bin/env python3
"""Lightweight x86 xref index over the relocated Masm_LE flat image.

Ghidra's auto-analysis under-resolves this Watcom code, so this gives a complete,
independent cross-reference: every E8 rel32 CALL / E9 rel32 JMP target, and every
32-bit immediate that points into the image (mov reg,imm32 / push imm32 / data
pointer). Use it to find callers of a routine or all references to a global/string.

Usage:
  masm_xref.py callers 0x1E1AE          # E8/E9 sites whose target == VA
  masm_xref.py refs    0x3A738          # any 32-bit immediate == VA (code+data)
  masm_xref.py word    0x330            # raw 16/32-bit value occurrences
"""
import struct
import sys

BASE = 0x10000
CODE_LO, CODE_HI = 0x10000, 0x2A1C4
DATA_LO, DATA_HI = 0x30000, 0x4BECF
DATA = open(__file__.rsplit("\\", 1)[0] + "\\masm_flat.bin", "rb").read()


def at(va):
    return va - BASE


def callers(target):
    """All E8/E9 rel32 sites in the code segment that branch to target."""
    out = []
    for i in range(at(CODE_LO), at(CODE_HI) - 5):
        op = DATA[i]
        if op in (0xE8, 0xE9):
            disp = struct.unpack_from("<i", DATA, i + 1)[0]
            site = i + BASE
            tgt = site + 5 + disp
            if tgt == target:
                out.append((site, "call" if op == 0xE8 else "jmp"))
    return out


def refs(target):
    """All 32-bit little-endian occurrences of target across the image."""
    needle = struct.pack("<I", target)
    out = []
    i = 0
    while True:
        j = DATA.find(needle, i)
        if j < 0:
            break
        out.append(j + BASE)
        i = j + 1
    return out


def seg(va):
    if CODE_LO <= va < CODE_HI:
        return "code"
    if DATA_LO <= va < DATA_HI:
        return "data"
    return "????"


def main():
    cmd = sys.argv[1]
    val = int(sys.argv[2], 0)
    if cmd == "callers":
        r = callers(val)
        print(f"{len(r)} call/jmp sites to {val:#x}:")
        for site, kind in r:
            print(f"  {site:#08x} {kind}")
    elif cmd == "refs":
        r = refs(val)
        print(f"{len(r)} immediate refs to {val:#x}:")
        for a in r:
            print(f"  {a:#08x} [{seg(a)}]")
    elif cmd == "word":
        for sz, fmt in ((2, "<H"), (4, "<I")):
            needle = struct.pack(fmt, val & ((1 << (sz * 8)) - 1))
            hits = []
            i = 0
            while True:
                j = DATA.find(needle, i)
                if j < 0:
                    break
                hits.append(j + BASE)
                i = j + 1
            hits = [h for h in hits if CODE_LO <= h < CODE_HI]
            print(f"{sz*8}-bit {val:#x}: {len(hits)} in code: " + " ".join(f"{h:#x}" for h in hits[:30]))


if __name__ == "__main__":
    main()
